//! Keyboard interruption and clean-exit shortcuts.

use super::{
    BrowserHandoff, ESCAPE_QUEUE_TIMEOUT, QUIT_SHORTCUT_TIMEOUT, TuiClient, TuiControl, UiAction,
};
use std::time::Instant;

impl<B: BrowserHandoff> TuiClient<B> {
    /// Interrupts active work or exits after a second press within the shortcut timeout.
    pub fn handle_ctrl_c(&mut self) -> TuiControl {
        if self.state.interruptible_submission().is_some() {
            self.core.cancel_current_submission();
            self.state.clear_quit_shortcut();
            self.state.clear_escape_shortcut();
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

    pub(in crate::tui) fn shutdown(&mut self) {
        if let Err(error) = self.core.shutdown() {
            self.state.add_error(error);
        }
    }

    pub(in crate::tui) fn exit_now(&mut self) -> TuiControl {
        self.core.cancel_current_submission();
        self.shutdown();
        self.state.clear_quit_shortcut();
        self.state.clear_escape_shortcut();
        self.state.reduce(UiAction::CancelAndExit);
        TuiControl::Exit
    }

    pub(in crate::tui) fn handle_escape(&mut self) {
        let now = Instant::now();
        let has_queue = self.state.queued_prompt_count() != 0;
        self.core.cancel_current_submission();
        if has_queue {
            let expires_at = now.checked_add(ESCAPE_QUEUE_TIMEOUT).unwrap_or(now);
            self.state.arm_escape_shortcut(expires_at);
        } else {
            self.state.clear_escape_shortcut();
        }
    }
}
