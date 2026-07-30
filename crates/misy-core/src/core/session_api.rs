//! Public session operations and persistence hooks over the on-disk store.

use super::{
    CoreError, CoreEvent, CoreState, HistoryEntry, MisyCore,
    session::{ResumeOutcome, SessionError, SessionSummary},
};
use crate::ModelRef;

impl MisyCore {
    /// Lists saved sessions belonging to the process working directory, newest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or local session storage cannot be read.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, CoreError> {
        self.inner.state.ensure_running()?;
        self.inner
            .state
            .session
            .lock()
            .map_err(|_| CoreError::from(SessionError::StatePoisoned))?
            .list_current_cwd()
            .map_err(Into::into)
    }

    /// Starts a clean conversation while retaining the previous persisted session.
    ///
    /// # Errors
    ///
    /// Returns an error when work is active, the core is shut down, or session state is poisoned.
    pub fn new_session(&self) -> Result<(), CoreError> {
        let _queue = self.lock_idle_session_queue()?;
        let workspace_cwd = self
            .inner
            .state
            .instructions
            .lock()
            .map_err(|_| CoreError::Runtime("instruction runtime is poisoned".to_owned()))?
            .root()
            .workspace_cwd()
            .to_path_buf();
        let instruction_root = super::instructions::InstructionRoot::load_at(
            &self.inner.state.misy_paths,
            true,
            &workspace_cwd,
        )
        .map_err(CoreError::InstructionBlocked)?;
        let mut history = self
            .inner
            .state
            .history
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut todos = self
            .inner
            .state
            .todos
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut active_history = self
            .inner
            .state
            .active_history
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut session = self
            .inner
            .state
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut instructions = self
            .inner
            .state
            .instructions
            .lock()
            .map_err(|_| CoreError::Runtime("instruction runtime is poisoned".to_owned()))?;
        history.clear();
        active_history.clear();
        todos.clear();
        session.detach();
        instructions.replace(instruction_root);
        Ok(())
    }

    /// Restores a saved conversation by exact id, filename, or unambiguous id prefix.
    ///
    /// # Errors
    ///
    /// Returns an error when work is active or the requested saved session cannot be loaded.
    pub fn resume_session(&self, id: &str) -> Result<ResumeOutcome, CoreError> {
        let _queue = self.lock_idle_session_queue()?;
        let workspace_cwd = self
            .inner
            .state
            .instructions
            .lock()
            .map_err(|_| CoreError::Runtime("instruction runtime is poisoned".to_owned()))?
            .root()
            .workspace_cwd()
            .to_path_buf();
        let instruction_root = super::instructions::InstructionRoot::load_at(
            &self.inner.state.misy_paths,
            true,
            &workspace_cwd,
        )
        .map_err(CoreError::InstructionBlocked)?;
        let loaded = self
            .inner
            .state
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?
            .load(id)?;
        let mut history = self
            .inner
            .state
            .history
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut todos = self
            .inner
            .state
            .todos
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut active_history = self
            .inner
            .state
            .active_history
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut session = self
            .inner
            .state
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let mut instructions = self
            .inner
            .state
            .instructions
            .lock()
            .map_err(|_| CoreError::Runtime("instruction runtime is poisoned".to_owned()))?;
        *history = loaded.history.clone();
        *active_history = loaded.active_history.clone();
        *todos = loaded.todos.clone();
        session.attach(&loaded);
        instructions.replace(instruction_root);
        drop(todos);
        drop(instructions);
        drop(session);
        drop(history);
        let warning = self.restore_saved_profile(
            loaded.summary.model.as_ref(),
            loaded.summary.thinking.as_deref(),
        );
        Ok(ResumeOutcome {
            session: loaded.summary,
            history_len: loaded.history.len(),
            history: loaded.history,
            compactions: loaded.compactions,
            todos: loaded.todos,
            model_warning: warning,
        })
    }

    /// Returns the id of the attached persisted session, if any history has been written.
    pub fn current_session_id(&self) -> Option<String> {
        self.inner
            .state
            .session
            .lock()
            .ok()
            .and_then(|session| session.current_id())
    }

    fn lock_idle_session_queue(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, super::queue::SubmissionQueue>, CoreError> {
        self.inner.state.ensure_running()?;
        let (live_agents, pending_results) = self.inner.state.agents.pending_state();
        if live_agents > 0 || pending_results > 0 || !self.inner.state.agents.list().is_empty() {
            return Err(CoreError::SessionAgentStatePending {
                live_agents,
                pending_results,
            });
        }
        let queue = self
            .inner
            .state
            .submission_queue
            .lock()
            .map_err(|_| SessionError::StatePoisoned)?;
        let (active, queued) = queue.snapshot();
        if active.is_some() || !queued.is_empty() {
            return Err(CoreError::SessionBusy);
        }
        Ok(queue)
    }

    fn restore_saved_profile(
        &self,
        saved: Option<&ModelRef>,
        saved_thinking: Option<&str>,
    ) -> Option<String> {
        let saved = saved?;
        let available = self.inner.state.model_cache.load();
        if let Some(info) = available.iter().find(|model| &model.model == saved) {
            let capability = self
                .inner
                .state
                .catalog
                .get(&saved.provider)
                .is_some_and(|package| package.manifest().supports_capability("thinking", 1));
            let supported_saved = saved_thinking.filter(|selected| {
                capability
                    && info.thinking.as_ref().is_some_and(|thinking| {
                        thinking.levels.iter().any(|level| level.id == *selected)
                    })
            });
            let effective_thinking = supported_saved.map(ToOwned::to_owned).or_else(|| {
                capability
                    .then(|| {
                        info.thinking
                            .as_ref()
                            .map(|thinking| thinking.default.clone())
                    })
                    .flatten()
            });
            if let Ok(mut selected) = self.inner.state.selected_model.lock() {
                *selected = Some(saved.clone());
            }
            if let Ok(mut selected) = self.inner.state.selected_thinking.lock() {
                *selected = effective_thinking;
            }
            self.emit(&CoreEvent::ModelSelected {
                model: saved.clone(),
            });
            return (saved_thinking.is_some() && supported_saved.is_none()).then(|| {
                format!(
                    "saved thinking level `{}` is unavailable; using the provider default",
                    saved_thinking.unwrap_or_default()
                )
            });
        }
        Some(format!(
            "saved model `{}/{}` is unavailable; keeping the current model",
            saved.provider.as_str(),
            saved.model.as_str()
        ))
    }
}

impl CoreState {
    pub(super) fn persist_todos(&self, todos: Vec<crate::TodoItem>) {
        let model = self
            .selected_model
            .lock()
            .ok()
            .and_then(|model| model.clone());
        let result = self
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)
            .and_then(|mut session| session.append_todos(todos, model));
        if let Err(error) = result {
            self.emit(&CoreEvent::SessionPersistenceFailed {
                message: error.to_string(),
            });
        }
    }
    pub(super) fn persist_history(&self, entry: HistoryEntry) {
        let model = self
            .selected_model
            .lock()
            .ok()
            .and_then(|model| model.clone());
        let result = self
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)
            .and_then(|mut session| session.append_history(entry, model));
        if let Err(error) = result {
            self.emit(&CoreEvent::SessionPersistenceFailed {
                message: error.to_string(),
            });
        }
    }

    pub(super) fn persist_model_change(&self, model: ModelRef) {
        let result = self
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)
            .and_then(|mut session| {
                let thinking = self
                    .selected_thinking
                    .lock()
                    .ok()
                    .and_then(|thinking| thinking.clone());
                session.append_model(model, thinking)
            });
        if let Err(error) = result {
            self.emit(&CoreEvent::SessionPersistenceFailed {
                message: error.to_string(),
            });
        }
    }

    pub(super) fn persist_thinking_change(&self, thinking: Option<String>) {
        let model = self
            .selected_model
            .lock()
            .ok()
            .and_then(|model| model.clone());
        let result = self
            .session
            .lock()
            .map_err(|_| SessionError::StatePoisoned)
            .and_then(|mut session| session.append_thinking(thinking, model));
        if let Err(error) = result {
            self.emit(&CoreEvent::SessionPersistenceFailed {
                message: error.to_string(),
            });
        }
    }
}
