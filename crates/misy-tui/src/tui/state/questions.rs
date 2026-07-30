//! Snapshot reconciliation and client actions for core-owned questions.

use super::{ActiveView, UiState};
use crate::tui::question_dialog::{InlineQuestionPresentation, QuestionDialog};
use misy_core::{CoreSnapshot, QuestionRequestId, QuestionResponse};
use std::time::Instant;

impl UiState {
    pub(in crate::tui) fn apply_snapshot(&mut self, snapshot: CoreSnapshot) {
        self.apply_snapshot_at(snapshot, Instant::now());
    }

    fn apply_snapshot_at(&mut self, snapshot: CoreSnapshot, now: Instant) {
        if self.snapshot.active_submission != snapshot.active_submission {
            self.capture_turn_transition(snapshot.active_submission, now);
        }
        if self.snapshot.compaction.is_none() && snapshot.compaction.is_some() {
            self.compaction_started_at = Some(now);
        } else if snapshot.compaction.is_none() {
            self.compaction_started_at = None;
        }
        let activities = snapshot.activities.clone();
        let agents = snapshot.agents.clone();
        self.snapshot = snapshot;
        self.reconcile_questions();
        if !self.activity_bar_visible() {
            self.activity_bar_focused = false;
        }
        match &mut self.view {
            Some(ActiveView::Activities(picker)) => picker.refresh(activities, agents),
            Some(ActiveView::ActivityLog(view)) => view.picker.refresh(activities, agents),
            _ => {}
        }
    }

    fn reconcile_questions(&mut self) {
        let current = match &self.view {
            Some(ActiveView::Question(dialog)) => Some(dialog.id()),
            _ => None,
        };
        if current.is_some_and(|id| {
            self.snapshot
                .pending_questions
                .iter()
                .any(|request| request.id == id)
        }) {
            return;
        }
        if let Some(request) = self.snapshot.pending_questions.first().cloned() {
            self.view = Some(ActiveView::Question(QuestionDialog::new(request)));
        } else if matches!(self.view, Some(ActiveView::Question(_))) {
            self.view = None;
        }
    }

    pub(in crate::tui) fn question_editing(&self) -> bool {
        matches!(&self.view, Some(ActiveView::Question(view)) if view.editing_other())
    }

    pub(in crate::tui) fn question_presentation(
        &self,
        visible_rows: usize,
    ) -> Option<InlineQuestionPresentation> {
        match &self.view {
            Some(ActiveView::Question(view)) => Some(view.inline_presentation(visible_rows)),
            _ => None,
        }
    }

    pub(in crate::tui) fn confirm_question(&mut self) -> Option<QuestionResponse> {
        match &mut self.view {
            Some(ActiveView::Question(view)) => view.confirm(),
            _ => None,
        }
    }

    pub(in crate::tui) fn choose_question_number(
        &mut self,
        one_based: usize,
    ) -> Option<QuestionResponse> {
        match &mut self.view {
            Some(ActiveView::Question(view)) => view.choose_number(one_based),
            _ => None,
        }
    }

    pub(in crate::tui) fn escape_question(&mut self) -> Option<QuestionRequestId> {
        match &mut self.view {
            Some(ActiveView::Question(view)) => view.escape().then(|| view.id()),
            _ => None,
        }
    }
}
