//! Main-submission wiring over the session-scoped turn loop.

use super::{ActiveSubmission, CoreEvent, CoreState, HistoryEntry, SubmissionId, turn};
use crate::{ImageAttachment, Message, ModelRef};
use serde_json::Value;
use std::sync::{Arc, atomic::Ordering};

impl CoreState {
    pub(super) async fn run_submission(
        &self,
        id: SubmissionId,
        model: &ModelRef,
        message: Message,
        attachments: Vec<ImageAttachment>,
        active: &Arc<ActiveSubmission>,
    ) -> CoreEvent {
        if active.cancelled.load(Ordering::Acquire) {
            return CoreEvent::Cancelled { submission: id };
        }
        self.emit(&CoreEvent::SubmissionStarted {
            submission: id,
            model: model.clone(),
        });
        if let Err(error) = self
            .auto_compact_if_needed(model, &message, &attachments, Arc::clone(active))
            .await
        {
            return CoreEvent::Failed {
                submission: id,
                message: format!("automatic compaction failed: {error}"),
            };
        }
        if !attachments.is_empty() {
            self.clear_main_history_images();
        }
        let role_catalog = super::roles::parent_catalog_description(&self.discover_roles());
        let state = turn::AgentTurnState::main(
            model.clone(),
            self.selected_thinking
                .lock()
                .expect("selected thinking mutex must not be poisoned")
                .clone(),
            Arc::clone(&self.active_history),
            Arc::clone(&self.todos),
            Arc::clone(active),
            id,
            self.instructions
                .lock()
                .expect("instruction runtime mutex must not be poisoned")
                .main(),
        )
        .with_role_catalog(role_catalog);
        state
            .instructions()
            .lock()
            .expect("instruction session mutex must not be poisoned")
            .begin_submission();
        self.push_main_history(HistoryEntry {
            message,
            attachments,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            provider_metadata: Value::Null,
        });
        let first_outcome = turn::run_turns(self, &state).await;
        let outcome = if let Err(error) = &first_outcome
            && error.is_context_limit()
            && error.allows_fallback()
        {
            let profile = crate::ModelProfile::new(
                state.model().clone(),
                state.thinking().map(ToOwned::to_owned),
            );
            match self
                .compact_after_overflow(&profile, Arc::clone(active))
                .await
            {
                Ok(()) => turn::run_turns(self, &state).await,
                Err(compaction) => Err(turn::TurnFailure::terminal(format!(
                    "overflow compaction failed: {compaction}"
                ))),
            }
        } else {
            first_outcome
        };
        let outcome = match outcome {
            Ok(()) => CoreEvent::Completed { submission: id },
            Err(error) if error.is_cancelled() || active.cancelled.load(Ordering::Acquire) => {
                CoreEvent::Cancelled { submission: id }
            }
            Err(error) => CoreEvent::Failed {
                submission: id,
                message: error.to_string(),
            },
        };
        state
            .instructions()
            .lock()
            .expect("instruction session mutex must not be poisoned")
            .finish_submission();
        outcome
    }

    fn push_main_history(&self, entry: HistoryEntry) {
        self.history
            .lock()
            .expect("history mutex must not be poisoned")
            .push(entry.clone());
        self.active_history
            .lock()
            .expect("active history mutex must not be poisoned")
            .push(entry.clone());
        self.persist_history(entry);
    }

    fn clear_main_history_images(&self) {
        let mut history = self
            .active_history
            .lock()
            .expect("history mutex must not be poisoned");
        for entry in &mut *history {
            entry.attachments.clear();
            for result in &mut entry.tool_results {
                result.attachments.clear();
            }
        }
    }

    pub(super) fn emit(&self, event: &CoreEvent) {
        self.subscribers.emit(event);
    }
}
