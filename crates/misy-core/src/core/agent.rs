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
        if !attachments.is_empty() {
            self.clear_main_history_images();
        }
        let state = turn::AgentTurnState::main(
            model.clone(),
            Arc::clone(&self.history),
            Arc::clone(active),
            id,
        );
        self.push_main_history(HistoryEntry {
            message,
            attachments,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            provider_metadata: Value::Null,
        });
        match turn::run_turns(self, &state).await {
            Ok(()) => CoreEvent::Completed { submission: id },
            Err(message) if message == "cancelled" || active.cancelled.load(Ordering::Acquire) => {
                CoreEvent::Cancelled { submission: id }
            }
            Err(message) => CoreEvent::Failed {
                submission: id,
                message,
            },
        }
    }

    fn push_main_history(&self, entry: HistoryEntry) {
        self.history
            .lock()
            .expect("history mutex must not be poisoned")
            .push(entry.clone());
        self.persist_history(entry);
    }

    fn clear_main_history_images(&self) {
        let mut history = self
            .history
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
