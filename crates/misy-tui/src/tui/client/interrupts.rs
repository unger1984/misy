//! Keyboard interruption and clean-exit shortcuts.

use super::{BrowserHandoff, QUIT_SHORTCUT_TIMEOUT, TuiClient, UiAction};
use misy_core::SubmissionId;
use std::time::Instant;

impl<B: BrowserHandoff> TuiClient<B> {
    /// Interrupts active work or exits after a second press within the shortcut timeout.
    ///
    /// Exit is flagged through [`UiState::should_exit`](crate::tui::UiState::should_exit); the
    /// terminal loop observes the state and leaves on its next iteration.
    pub fn handle_ctrl_c(&mut self) {
        self.refresh_core_projection();
        if self.state.compaction_is_cancellable() {
            let core = self.core.clone();
            tokio::spawn(async move {
                core.cancel_compaction().await;
            });
            self.state.clear_quit_shortcut();
            return;
        }
        if let Some(submission) = self.state.interruptible_submission() {
            let core = self.core.clone();
            tokio::spawn(async move {
                // A failed cancel means the submission already finished; its terminal event
                // settles the UI either way.
                let _ = core.cancel(submission).await;
            });
            self.state.clear_quit_shortcut();
            return;
        }
        let now = Instant::now();
        if self.state.quit_shortcut_active(now) {
            self.exit_now();
            return;
        }
        let expires_at = now.checked_add(QUIT_SHORTCUT_TIMEOUT).unwrap_or(now);
        self.state.arm_quit_shortcut(expires_at);
    }

    pub(in crate::tui) async fn shutdown(&mut self) {
        if let Err(error) = self.core.shutdown().await {
            self.state.add_error(error);
        }
    }

    /// Flags exit in the UI state and shuts the core down in the background.
    pub(in crate::tui) fn exit_now(&mut self) {
        let core = self.core.clone();
        tokio::spawn(async move {
            core.cancel_current_submission().await;
            // The exit is already flagged, so a shutdown failure cannot change the outcome.
            let _ = core.shutdown().await;
        });
        self.state.clear_quit_shortcut();
        self.state.reduce(&UiAction::CancelAndExit);
    }

    pub(in crate::tui) fn handle_escape(&mut self, submission: SubmissionId) {
        let core = self.core.clone();
        tokio::spawn(async move {
            // A failed cancel means the submission already finished; its terminal event
            // settles the UI either way.
            let _ = core.cancel(submission).await;
        });
    }
}
