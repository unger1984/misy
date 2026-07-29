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
        *todos = loaded.todos.clone();
        session.attach(&loaded);
        instructions.replace(instruction_root);
        drop(todos);
        drop(instructions);
        drop(session);
        drop(history);
        let warning = self.restore_saved_model(loaded.summary.model.as_ref());
        Ok(ResumeOutcome {
            session: loaded.summary,
            history_len: loaded.history.len(),
            history: loaded.history,
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

    fn restore_saved_model(&self, saved: Option<&ModelRef>) -> Option<String> {
        let saved = saved?;
        let available = self.inner.state.model_cache.load();
        if available.iter().any(|model| &model.model == saved) {
            if let Ok(mut selected) = self.inner.state.selected_model.lock() {
                *selected = Some(saved.clone());
            }
            self.emit(&CoreEvent::ModelSelected {
                model: saved.clone(),
            });
            return None;
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
            .and_then(|mut session| session.append_model(model));
        if let Err(error) = result {
            self.emit(&CoreEvent::SessionPersistenceFailed {
                message: error.to_string(),
            });
        }
    }
}
