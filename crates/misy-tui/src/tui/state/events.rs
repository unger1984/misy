//! Core-event projection into transcript rows.

use super::{TranscriptRow, UiState};
use misy_core::{CoreEvent, SubmissionId, ToolResult};

impl UiState {
    pub(in crate::tui) fn apply_core_event(&mut self, event: CoreEvent) {
        if let Some(submission) = event_submission(&event)
            && !self.prompt_text.contains_key(&submission.get())
        {
            self.pending_submission_events
                .entry(submission.get())
                .or_default()
                .push(event);
            return;
        }
        self.apply_mapped_core_event(event);
    }

    pub(in crate::tui) fn apply_mapped_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ProviderDiscovered { .. } | CoreEvent::ModelsListed { .. } => {}
            CoreEvent::AuthenticationChanged { .. } | CoreEvent::ModelSelected { .. } => {}
            CoreEvent::SubmissionStarted { submission, .. } => self.start_submission(submission),
            CoreEvent::TextDelta {
                submission, delta, ..
            } => {
                self.response_started = true;
                self.append_assistant_text(Some(submission), delta);
            }
            CoreEvent::ToolCall { call, .. } => self.transcript.push(TranscriptRow::ToolCall {
                id: call.id,
                name: call.name,
                arguments: serde_json::to_string(&call.arguments).ok(),
            }),
            CoreEvent::ToolResult { result, .. } => self.add_tool_result(result),
            CoreEvent::Completed { .. } => {}
            CoreEvent::Cancelled { submission } => self.note_cancellation(submission),
            CoreEvent::Failed { message, .. } => self.add_error(message),
            CoreEvent::Shutdown => self.should_exit = true,
        }
    }

    fn add_tool_result(&mut self, result: ToolResult) {
        self.transcript.push(TranscriptRow::ToolResult {
            id: result.tool_call_id,
            is_error: result.is_error,
            content: Some(result.content),
        });
    }

    fn note_cancellation(&mut self, submission: SubmissionId) {
        if !self.cancelled_submissions.insert(submission.get()) {
            return;
        }
        self.add_info("submission cancelled");
    }

    pub(in crate::tui) fn append_assistant_text(
        &mut self,
        submission: Option<SubmissionId>,
        text: String,
    ) {
        if self.response_submission == submission
            && let Some(TranscriptRow::AssistantText(previous)) = self.transcript.last_mut()
        {
            previous.push_str(&text);
            return;
        }
        self.transcript.push(TranscriptRow::AssistantText(text));
        self.response_submission = submission;
    }
}

fn event_submission(event: &CoreEvent) -> Option<SubmissionId> {
    match event {
        CoreEvent::SubmissionStarted { submission, .. }
        | CoreEvent::TextDelta { submission, .. }
        | CoreEvent::ToolCall { submission, .. }
        | CoreEvent::ToolResult { submission, .. }
        | CoreEvent::Completed { submission }
        | CoreEvent::Cancelled { submission }
        | CoreEvent::Failed { submission, .. } => Some(*submission),
        CoreEvent::ProviderDiscovered { .. }
        | CoreEvent::AuthenticationChanged { .. }
        | CoreEvent::ModelsListed { .. }
        | CoreEvent::ModelSelected { .. }
        | CoreEvent::Shutdown => None,
    }
}
