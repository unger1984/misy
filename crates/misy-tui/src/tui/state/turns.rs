//! Transcript and presentation derived from the core submission snapshot.

use super::{TranscriptRow, UiState};
use misy_core::SubmissionId;
use std::time::Instant;

impl UiState {
    /// Returns the submission currently occupying the core-owned FIFO slot.
    pub fn active_submission(&self) -> Option<SubmissionId> {
        self.snapshot.active_submission
    }

    pub(in crate::tui) fn interruptible_submission(&self) -> Option<SubmissionId> {
        self.active_submission()
    }

    pub(in crate::tui) fn queue_prompt_submission(&mut self, text: String) {
        self.pending_prompt_text.push_back(text);
    }

    pub(in crate::tui) fn discard_last_pending_submission(&mut self) {
        self.pending_prompt_text.pop_back();
    }

    pub(in crate::tui) fn reject_submission(&mut self, text: &str) {
        let rejected = self.pending_prompt_text.pop_front();
        debug_assert_eq!(rejected.as_deref(), Some(text));
    }

    pub(in crate::tui) fn accept_submission(&mut self, submission: SubmissionId, text: String) {
        let accepted = self.pending_prompt_text.pop_front();
        debug_assert_eq!(accepted.as_deref(), Some(text.as_str()));
        self.prompt_text.insert(submission.get(), text);
        if let Some(events) = self.pending_submission_events.remove(&submission.get()) {
            for event in events {
                self.apply_mapped_core_event(event);
            }
        }
    }

    pub(in crate::tui) fn busy_label(&self, now: Instant) -> Option<String> {
        let started = self.submission_started_at?;
        let elapsed = now.saturating_duration_since(started);
        let phase = if self.response_started {
            "Responding…"
        } else {
            "Thinking…"
        };
        Some(format!(
            "{} {phase} ({}s · esc to interrupt)",
            spinner_frame(elapsed.as_millis() / 100),
            elapsed.as_secs()
        ))
    }

    pub(in crate::tui) fn queued_prompt_lines(&self, maximum: usize) -> Vec<String> {
        let queued = self
            .snapshot
            .queued_submissions
            .iter()
            .filter_map(|submission| self.prompt_text.get(&submission.get()))
            .chain(self.pending_prompt_text.iter())
            .collect::<Vec<_>>();
        let mut lines = queued
            .iter()
            .take(maximum)
            .map(|text| format!("  ↳ {}", text.replace('\n', " ")))
            .collect::<Vec<_>>();
        let hidden = queued.len().saturating_sub(lines.len());
        if hidden != 0 {
            lines.push(format!("    … {hidden} more queued"));
        }
        lines
    }

    pub(in crate::tui) fn start_submission(&mut self, submission: SubmissionId) {
        if self.started_submissions.insert(submission.get())
            && let Some(text) = self.prompt_text.get(&submission.get())
        {
            self.transcript
                .push(TranscriptRow::UserPrompt(text.clone()));
        }
        self.response_submission = Some(submission);
    }
}

fn spinner_frame(ticks: u128) -> &'static str {
    match ticks % 10 {
        0 => "⠋",
        1 => "⠙",
        2 => "⠹",
        3 => "⠸",
        4 => "⠼",
        5 => "⠴",
        6 => "⠦",
        7 => "⠧",
        8 => "⠇",
        _ => "⠏",
    }
}

#[cfg(test)]
mod tests {
    use super::UiState;

    #[test]
    fn rejected_submission_removes_only_its_pending_preview() {
        let mut state = UiState::default();
        state.queue_prompt_submission("first".to_owned());
        state.queue_prompt_submission("second".to_owned());

        state.reject_submission("first");

        assert_eq!(state.queued_prompt_lines(3), ["  ↳ second"]);
    }
}
