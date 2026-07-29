//! Saved-session picker state kept outside the near-limit parent module.

use super::{ActiveView, SessionPicker, UiState};

impl UiState {
    pub(in crate::tui) fn open_sessions(&mut self, sessions: Vec<misy_core::SessionSummary>) {
        self.view = Some(ActiveView::Sessions(SessionPicker::new(
            sessions,
            self.session_id.as_deref(),
        )));
    }

    pub(in crate::tui) fn set_session_id(&mut self, id: Option<String>) {
        self.session_id = id;
    }

    pub(in crate::tui) fn selected_session_id(&self) -> Option<String> {
        match &self.view {
            Some(ActiveView::Sessions(view)) => view.selected_id().cloned(),
            _ => None,
        }
    }
}
