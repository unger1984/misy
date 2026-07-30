//! Scroll state for the content-specific `/context` popup.

use misy_core::ContextReport;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContextView {
    pub(super) report: ContextReport,
    pub(super) scroll: usize,
    pub(super) scroll_from_end: bool,
}

impl ContextView {
    pub(super) fn new(report: ContextReport) -> Self {
        Self {
            report,
            scroll: 0,
            scroll_from_end: false,
        }
    }

    pub(super) fn replace(&mut self, report: ContextReport) {
        self.report = report;
    }

    pub(super) fn up(&mut self, amount: usize) {
        if self.scroll_from_end {
            self.scroll = self.scroll.saturating_add(amount);
        } else {
            self.scroll = self.scroll.saturating_sub(amount);
        }
    }

    pub(super) fn down(&mut self, amount: usize) {
        if self.scroll_from_end {
            self.scroll = self.scroll.saturating_sub(amount);
        } else {
            self.scroll = self.scroll.saturating_add(amount);
        }
    }

    pub(super) fn home(&mut self) {
        self.scroll = 0;
        self.scroll_from_end = false;
    }

    pub(super) fn end(&mut self) {
        self.scroll = 0;
        self.scroll_from_end = true;
    }

    pub(super) fn effective_scroll(&self, maximum: usize) -> usize {
        if self.scroll_from_end {
            maximum.saturating_sub(self.scroll)
        } else {
            self.scroll.min(maximum)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContextView;
    use misy_core::{ContextReport, ContextReportState};

    #[test]
    fn page_up_from_end_is_relative_to_the_rendered_maximum() {
        let mut view = ContextView::new(ContextReport {
            model: None,
            model_display_name: None,
            context_window: 0,
            estimated_tokens: 0,
            categories: Vec::new(),
            state: ContextReportState::Base,
            sources: Vec::new(),
            image_count: 0,
            image_bytes: 0,
            warnings: Vec::new(),
            roles: Vec::new(),
            auto_compaction_threshold: None,
            compaction_reserve_tokens: None,
        });
        view.end();
        assert_eq!(view.effective_scroll(80), 80);
        view.up(10);
        assert_eq!(view.effective_scroll(80), 70);
    }
}
