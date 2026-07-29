//! Atomic state transitions for one child-agent record.

use super::registry::{AgentRecord, InboxDecision, now_millis};
use crate::{
    ActivityStatus, AgentSummary, AgentTranscript, CoreError, HistoryEntry,
    InstructionSourceSummary, MessageRole, core::ActiveSubmission,
};
use std::{sync::Arc, time::Duration};

impl AgentRecord {
    pub(super) fn summary(&self) -> AgentSummary {
        self.state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .summary
            .clone()
    }

    pub(super) fn transcript(&self) -> AgentTranscript {
        let state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        state.transcript.snapshot(state.summary.clone())
    }

    pub(super) async fn wait_terminal(&self, duration: Duration) -> bool {
        let changed = self.terminal_changed.notified();
        tokio::pin!(changed);
        if self.summary().status.is_terminal() {
            return true;
        }
        tokio::time::timeout(duration, &mut changed).await.is_ok()
            && self.summary().status.is_terminal()
    }

    pub(super) fn final_result(&self) -> Option<String> {
        self.state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .final_result
            .clone()
    }

    pub(super) fn cancel(&self) {
        if let Some(active) = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .active
            .as_ref()
        {
            active.cancel();
        }
    }

    pub(super) fn attach_active(&self, active: Arc<ActiveSubmission>) {
        self.state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .active = Some(active);
    }

    pub(super) fn message(&self, message: &str) -> Result<(), CoreError> {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        if state.summary.status.is_terminal() || !state.inbox_open {
            return Err(CoreError::AgentAlreadyFinished(state.summary.id));
        }
        state.inbox.push_back(message.to_owned());
        state.transcript.push_user_message(message);
        Ok(())
    }

    pub(super) fn decide_inbox(&self, can_continue: bool) -> InboxDecision {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        if can_continue && !state.inbox.is_empty() {
            return InboxDecision::Continue(state.inbox.drain(..).collect());
        }
        state.inbox_open = false;
        InboxDecision::Close
    }

    pub(super) fn capture_history(&self, entries: &[HistoryEntry]) {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        for entry in entries {
            if entry.message.role == MessageRole::Assistant && !entry.message.content.is_empty() {
                state.transcript.push_assistant(&entry.message.content);
            }
            for call in &entry.tool_calls {
                state.transcript.push_tool_call(call);
            }
            for result in &entry.tool_results {
                state.transcript.push_tool_result(result);
            }
        }
    }

    pub(super) fn set_instruction_sources(&self, sources: Vec<InstructionSourceSummary>) {
        self.state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .transcript
            .set_instruction_sources(sources);
    }

    pub(super) fn finish(&self, status: ActivityStatus, result: &str) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        if state.summary.status.is_terminal() {
            return false;
        }
        state.inbox_open = false;
        state.inbox.clear();
        state.summary.status = status;
        state.summary.finished_at_ms = Some(now_millis());
        state.summary.terminal_message = Some(result.to_owned());
        state.final_result = Some(result.to_owned());
        state.transcript.push_terminal(result);
        state.live_permit.take();
        state.active = None;
        self.terminal_changed.notify_waiters();
        true
    }

    pub(super) fn mark_mailbox_pending(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        if state.mailbox_permit.is_none() {
            return false;
        }
        state.mailbox_pending = true;
        true
    }

    pub(super) fn consume_mailbox(&self) {
        let mut state = self
            .state
            .lock()
            .expect("agent record mutex must not be poisoned");
        state.mailbox_pending = false;
        state.mailbox_permit.take();
    }

    pub(super) fn mailbox_pending(&self) -> bool {
        self.state
            .lock()
            .expect("agent record mutex must not be poisoned")
            .mailbox_pending
    }
}
