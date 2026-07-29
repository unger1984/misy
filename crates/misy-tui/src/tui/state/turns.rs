//! Transcript and presentation derived from the core submission snapshot.

use super::{TranscriptRow, UiState, spinner_frame};
use misy_core::SubmissionId;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalTurn {
    pub(super) submission: SubmissionId,
    pub(super) elapsed: Duration,
    pub(super) had_tool_activity: bool,
}

impl UiState {
    pub(super) fn capture_turn_transition(
        &mut self,
        next_submission: Option<SubmissionId>,
        now: Instant,
    ) {
        if let Some(submission) = self.snapshot.active_submission {
            let elapsed = self
                .submission_started_at
                .map_or(Duration::ZERO, |started| {
                    now.saturating_duration_since(started)
                });
            self.terminal_turn = Some(TerminalTurn {
                submission,
                elapsed,
                had_tool_activity: self.turn_had_tool_activity,
            });
        }
        self.submission_started_at = next_submission.map(|_| now);
        self.turn_had_tool_activity = false;
        self.response_started = false;
    }

    /// Returns the submission currently occupying the core-owned FIFO slot.
    pub fn active_submission(&self) -> Option<SubmissionId> {
        self.snapshot.active_submission
    }

    pub(in crate::tui) fn interruptible_submission(&self) -> Option<SubmissionId> {
        self.active_submission()
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
        // The core emits `SubmissionStarted` exactly once per submission, and the acceptance
        // event has already supplied the prompt text, so no dedup or replay guard is needed.
        if let Some(text) = self.prompt_text.get(&submission.get()) {
            self.transcript
                .push(TranscriptRow::UserPrompt(text.clone()));
        }
        self.response_submission = Some(submission);
        if self.active_submission() == Some(submission) {
            self.submission_started_at.get_or_insert_with(Instant::now);
            self.turn_had_tool_activity = false;
        }
    }
}
