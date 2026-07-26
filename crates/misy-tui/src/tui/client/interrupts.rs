//! Keyboard interruption and clean-exit shortcuts.

use super::{BrowserHandoff, QUIT_SHORTCUT_TIMEOUT, TuiClient, TuiControl, UiAction};
use misy_core::SubmissionId;
use std::time::Instant;

impl<B: BrowserHandoff> TuiClient<B> {
    /// Interrupts active work or exits after a second press within the shortcut timeout.
    pub fn handle_ctrl_c(&mut self) -> TuiControl {
        self.refresh_snapshot();
        if let Some(submission) = self.state.interruptible_submission() {
            let core = self.core.clone();
            tokio::spawn(async move {
                let _ = core.cancel(submission).await;
            });
            self.state.clear_quit_shortcut();
            return TuiControl::Continue;
        }
        let now = Instant::now();
        if self.state.quit_shortcut_active(now) {
            return self.exit_now();
        }
        let expires_at = now.checked_add(QUIT_SHORTCUT_TIMEOUT).unwrap_or(now);
        self.state.arm_quit_shortcut(expires_at);
        TuiControl::Continue
    }

    pub(in crate::tui) async fn shutdown(&mut self) {
        if let Err(error) = self.core.shutdown().await {
            self.state.add_error(error);
        }
    }

    pub(in crate::tui) fn exit_now(&mut self) -> TuiControl {
        let core = self.core.clone();
        tokio::spawn(async move {
            core.cancel_current_submission().await;
            let _ = core.shutdown().await;
        });
        self.state.clear_quit_shortcut();
        self.state.reduce(UiAction::CancelAndExit);
        TuiControl::Exit
    }

    pub(in crate::tui) fn handle_escape(&mut self, submission: SubmissionId) {
        let core = self.core.clone();
        tokio::spawn(async move {
            let _ = core.cancel(submission).await;
        });
    }
}
