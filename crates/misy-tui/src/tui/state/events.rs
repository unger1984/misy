//! Core-event projection into transcript rows.

use super::{TranscriptRow, UiState};
use misy_core::{CoreEvent, Message, SubmissionId, ToolResult};

impl UiState {
    pub(in crate::tui) fn apply_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ProviderDiscovered { .. } | CoreEvent::ModelsListed { .. } => {}
            CoreEvent::AuthenticationChanged { .. } | CoreEvent::ModelSelected { .. } => {}
            CoreEvent::SubmissionAccepted {
                submission,
                message,
            } => self.note_acceptance(submission, message),
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
                // Serializing a `Value` cannot fail in practice; `None` renders the row
                // without arguments.
                arguments: serde_json::to_string(&call.arguments).ok(),
            }),
            CoreEvent::ToolResult { result, .. } => self.add_tool_result(result),
            CoreEvent::Completed { .. } => {}
            CoreEvent::Cancelled { submission } => self.note_cancellation(submission),
            CoreEvent::Failed { message, .. } => self.add_error(message),
            CoreEvent::Shutdown => self.should_exit = true,
        }
    }

    // `SubmissionAccepted` always precedes every other event for its submission, so the prompt
    // text is already mapped when the start, stream, and terminal events arrive; no client-side
    // reordering buffer is needed.
    fn note_acceptance(&mut self, submission: SubmissionId, message: Message) {
        self.prompt_text.insert(submission.get(), message.content);
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
