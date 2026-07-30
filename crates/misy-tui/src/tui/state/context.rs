//! Context-popup state transitions and scroll navigation.

use super::{ActiveView, UiState};
use crate::tui::context_view::ContextView;
use misy_core::ContextReport;

impl UiState {
    pub(in crate::tui) fn open_context(&mut self, report: ContextReport) {
        self.view = Some(ActiveView::Context(ContextView::new(report)));
    }

    pub(in crate::tui) fn refresh_context(&mut self, report: ContextReport) {
        if let Some(ActiveView::Context(view)) = &mut self.view {
            view.replace(report);
        }
    }

    pub(in crate::tui) fn context_view(&self) -> Option<&ContextView> {
        match &self.view {
            Some(ActiveView::Context(view)) => Some(view),
            _ => None,
        }
    }

    pub(in crate::tui) fn scroll_context_page(&mut self, down: bool) {
        if let Some(ActiveView::Context(view)) = &mut self.view {
            if down {
                view.down(10);
            } else {
                view.up(10);
            }
        }
    }

    pub(in crate::tui) fn scroll_context_edge(&mut self, end: bool) {
        if let Some(ActiveView::Context(view)) = &mut self.view {
            if end {
                view.end();
            } else {
                view.home();
            }
        }
    }
}
