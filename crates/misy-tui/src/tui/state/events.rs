//! Core-event projection into transcript rows.

use super::{TranscriptRow, UiState};
use misy_core::{ActivityOutput, CoreEvent, SubmissionId, ToolResult};

impl UiState {
    pub(in crate::tui) fn apply_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ActivityChanged { .. } => {}
            CoreEvent::ActivityFinished { output } => self.add_activity_finished(output),
            CoreEvent::ProviderDiscovered { .. } | CoreEvent::ModelsListed { .. } => {}
            CoreEvent::AuthenticationChanged { .. } | CoreEvent::ModelSelected { .. } => {}
            CoreEvent::SubmissionAccepted {
                submission,
                message,
                attachment_count,
            } => self.note_acceptance(submission, &message.content, attachment_count),
            CoreEvent::SubmissionStarted { submission, .. } => self.start_submission(submission),
            CoreEvent::TextDelta {
                submission, delta, ..
            } => {
                self.response_started = true;
                self.append_assistant_text(Some(submission), delta);
            }
            CoreEvent::ToolCall {
                submission, call, ..
            } => {
                if self.active_submission() == Some(submission) {
                    self.turn_had_tool_activity = true;
                }
                self.transcript.push(TranscriptRow::ToolCall {
                    id: call.id,
                    name: call.name,
                    // Serializing a `Value` cannot fail in practice; `None` renders the row
                    // without arguments.
                    arguments: serde_json::to_string(&call.arguments).ok(),
                });
            }
            CoreEvent::ToolResult { result, .. } => self.add_tool_result(result),
            CoreEvent::Completed { submission } => self.complete_turn(submission),
            CoreEvent::Cancelled { submission } => {
                self.discard_terminal_turn(submission);
                self.note_cancellation(submission);
            }
            CoreEvent::Failed {
                submission,
                message,
            } => {
                self.discard_terminal_turn(submission);
                self.add_error(message);
            }
            CoreEvent::SessionPersistenceFailed { message } => {
                self.add_error(format!("could not save session: {message}"));
            }
            CoreEvent::Shutdown => {
                self.terminal_turn = None;
                self.should_exit = true;
            }
        }
    }

    // `SubmissionAccepted` always precedes every other event for its submission, so the prompt
    // text is already mapped when the start, stream, and terminal events arrive; no client-side
    // reordering buffer is needed.
    fn note_acceptance(
        &mut self,
        submission: SubmissionId,
        message: &str,
        attachment_count: usize,
    ) {
        let mut display = (1..=attachment_count)
            .map(|index| format!("[Image #{index}]"))
            .collect::<Vec<_>>()
            .join(" ");
        if !message.is_empty() {
            if !display.is_empty() {
                display.push(' ');
            }
            display.push_str(message);
        }
        self.prompt_text.insert(submission.get(), display);
    }

    fn add_tool_result(&mut self, result: ToolResult) {
        self.transcript.push(TranscriptRow::ToolResult {
            id: result.tool_call_id,
            is_error: result.is_error,
            content: Some(result.content),
        });
    }

    fn add_activity_finished(&mut self, output: ActivityOutput) {
        self.transcript
            .push(TranscriptRow::ActivityFinished(output));
    }

    fn note_cancellation(&mut self, submission: SubmissionId) {
        if !self.cancelled_submissions.insert(submission.get()) {
            return;
        }
        self.add_info("submission cancelled");
    }

    fn complete_turn(&mut self, submission: SubmissionId) {
        let Some(turn) = self.terminal_turn.take() else {
            return;
        };
        if turn.submission == submission && turn.had_tool_activity {
            self.transcript.push(TranscriptRow::WorkSeparator {
                elapsed: turn.elapsed,
            });
        }
    }

    fn discard_terminal_turn(&mut self, _submission: SubmissionId) {
        // Cancelled, failed, and mismatched terminal events all invalidate the one captured turn.
        self.terminal_turn = None;
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
