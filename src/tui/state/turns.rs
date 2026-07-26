//! Prompt queue and active-turn presentation state.

use super::{QueuedPrompt, TranscriptRow, UiState};
use crate::SubmissionId;
use std::time::Instant;

impl UiState {
    /// Returns the accepted or started submission currently owning the UI turn.
    pub fn active_submission(&self) -> Option<SubmissionId> {
        self.active_submission.or(self.starting_submission)
    }

    pub(in crate::tui) fn interruptible_submission(&self) -> Option<SubmissionId> {
        self.active_submission()
    }

    pub(in crate::tui) fn queued_prompt_count(&self) -> usize {
        self.queued_prompts.len()
    }

    pub(in crate::tui) fn clear_queued_prompts(&mut self) {
        self.queued_prompts.clear();
    }

    pub(in crate::tui) fn accept_submission(&mut self, submission: SubmissionId, text: String) {
        if self.interruptible_submission().is_none() && self.queued_prompts.is_empty() {
            self.transcript.push(TranscriptRow::UserPrompt(text));
            self.starting_submission = Some(submission);
            self.submission_started_at = Some(Instant::now());
            self.response_started = false;
        } else {
            self.queue_prompt(submission, text);
        }
    }

    pub(in crate::tui) fn arm_escape_shortcut(&mut self, expires_at: Instant) {
        self.escape_shortcut_expires_at = Some(expires_at);
    }

    pub(in crate::tui) fn clear_escape_shortcut(&mut self) {
        self.escape_shortcut_expires_at = None;
    }

    pub(in crate::tui) fn escape_shortcut_active(&self, now: Instant) -> bool {
        self.escape_shortcut_expires_at
            .is_some_and(|expires_at| now < expires_at)
    }

    pub(in crate::tui) fn set_active_submission(&mut self, submission: Option<SubmissionId>) {
        self.active_submission = submission;
        self.submission_started_at = self.active_submission.as_ref().map(|_| Instant::now());
        self.response_started = false;
    }

    pub(in crate::tui) fn busy_label(&self, now: Instant) -> Option<String> {
        let started = self.submission_started_at?;
        let elapsed = now.saturating_duration_since(started);
        let phase = if self.response_started {
            "Responding…"
        } else {
            "Thinking…"
        };
        let escape_hint = if self.escape_shortcut_active(now) {
            "esc again to stop queue"
        } else {
            "esc to interrupt"
        };
        Some(format!(
            "{} {phase} ({}s · {escape_hint})",
            spinner_frame(elapsed.as_millis() / 100),
            elapsed.as_secs()
        ))
    }

    pub(in crate::tui) fn queued_prompt_lines(&self, maximum: usize) -> Vec<String> {
        let mut lines = self
            .queued_prompts
            .iter()
            .take(maximum)
            .map(|prompt| format!("  ↳ {}", prompt.text.replace('\n', " ")))
            .collect::<Vec<_>>();
        let hidden = self.queued_prompts.len().saturating_sub(maximum);
        if hidden != 0 {
            lines.push(format!("    … {hidden} more queued"));
        }
        lines
    }

    pub(in crate::tui) fn finish_submission(&mut self, submission: SubmissionId) {
        if self.active_submission == Some(submission) {
            self.set_active_submission(None);
        }
    }

    fn queue_prompt(&mut self, submission: SubmissionId, text: String) {
        if self.active_submission == Some(submission)
            || self
                .queued_prompts
                .iter()
                .any(|prompt| prompt.submission == submission)
        {
            return;
        }
        self.queued_prompts
            .push_back(QueuedPrompt { submission, text });
    }

    pub(in crate::tui) fn start_submission(&mut self, submission: SubmissionId) {
        if self.active_submission == Some(submission) {
            return;
        }
        if self.starting_submission == Some(submission) {
            self.starting_submission = None;
            self.set_active_submission(Some(submission));
            return;
        }
        let Some(index) = self
            .queued_prompts
            .iter()
            .position(|prompt| prompt.submission == submission)
        else {
            self.set_active_submission(Some(submission));
            return;
        };
        if let Some(prompt) = self.queued_prompts.remove(index) {
            self.transcript.push(TranscriptRow::UserPrompt(prompt.text));
        }
        self.set_active_submission(Some(submission));
    }

    pub(in crate::tui) fn remove_queued_prompt(&mut self, submission: SubmissionId) {
        if self.starting_submission == Some(submission) {
            self.starting_submission = None;
            self.submission_started_at = None;
            self.response_started = false;
        }
        self.queued_prompts
            .retain(|prompt| prompt.submission != submission);
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
