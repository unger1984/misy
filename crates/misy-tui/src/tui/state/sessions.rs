//! Saved-session picker state kept outside the near-limit parent module.

use super::{ActiveView, SessionPicker, UiState};
use crate::tui::list::{ListRow, ListView};

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

    pub(in crate::tui) fn confirm_agent_discard(&mut self, live: usize, pending: usize) {
        let detail = format!("{live} live agent(s), {pending} unconsumed result(s)");
        self.view = Some(ActiveView::AgentDiscard(ListView::new(
            "Discard agent state?",
            vec![
                ListRow::selectable(false, "Keep agent state", Some(detail.clone())),
                ListRow::selectable(true, "Stop agents and discard", Some(detail)),
            ],
        )));
    }

    pub(in crate::tui) fn selected_agent_discard(&self) -> Option<bool> {
        match &self.view {
            Some(ActiveView::AgentDiscard(view)) => view.selected_value().copied(),
            _ => None,
        }
    }
}
