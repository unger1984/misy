//! Deterministic state for one core-owned structured question request.

use super::list::ListRowDisplay;
use misy_core::{QuestionRequest, QuestionRequestId, QuestionResponse};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuestionPage {
    cursor: usize,
    selected: BTreeSet<usize>,
    other_selected: bool,
    other_draft: String,
    editing_other: bool,
    complete: bool,
}

impl QuestionPage {
    fn new() -> Self {
        Self {
            cursor: 0,
            selected: BTreeSet::new(),
            other_selected: false,
            other_draft: String::new(),
            editing_other: false,
            complete: false,
        }
    }
}

/// Preserves selections and custom text while the user moves between question tabs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QuestionDialog {
    request: QuestionRequest,
    active: usize,
    pages: Vec<QuestionPage>,
}

/// Render data for the question surface that occupies the composer slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InlineQuestionPresentation {
    pub(super) title: String,
    pub(super) rows: Vec<ListRowDisplay>,
    pub(super) tabs: Vec<(String, bool)>,
    pub(super) help_hint: String,
    pub(super) submit_page: bool,
    pub(super) submit_ready: bool,
}

impl QuestionDialog {
    pub(super) fn new(request: QuestionRequest) -> Self {
        let pages = request
            .questions
            .iter()
            .map(|_| QuestionPage::new())
            .collect();
        Self {
            request,
            active: 0,
            pages,
        }
    }

    pub(super) fn id(&self) -> QuestionRequestId {
        self.request.id
    }

    pub(super) fn editing_other(&self) -> bool {
        !self.is_submit_page() && self.page().editing_other
    }

    pub(super) fn move_up(&mut self) {
        if self.is_submit_page() {
            return;
        }
        let row_count = self.row_count();
        let page = self.page_mut();
        page.cursor = if page.cursor == 0 {
            row_count.saturating_sub(1)
        } else {
            page.cursor - 1
        };
    }

    pub(super) fn move_down(&mut self) {
        if self.is_submit_page() {
            return;
        }
        let row_count = self.row_count();
        let page = self.page_mut();
        page.cursor = (page.cursor + 1) % row_count.max(1);
    }

    pub(super) fn tab_left(&mut self) {
        let tab_count = self.tab_count();
        self.active = if self.active == 0 {
            tab_count.saturating_sub(1)
        } else {
            self.active - 1
        };
    }

    pub(super) fn tab_right(&mut self) {
        self.active = (self.active + 1) % self.tab_count().max(1);
    }

    pub(super) fn select_number(&mut self, one_based: usize) -> bool {
        if self.is_submit_page() {
            return false;
        }
        let Some(index) = one_based.checked_sub(1) else {
            return false;
        };
        if index >= self.row_count() {
            return false;
        }
        self.page_mut().cursor = index;
        true
    }

    pub(super) fn choose_number(&mut self, one_based: usize) -> Option<QuestionResponse> {
        if !self.select_number(one_based) {
            return None;
        }
        if self.question().multi_select {
            self.toggle_current();
            None
        } else {
            self.confirm()
        }
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        if self.is_submit_page() {
            return;
        }
        if self.page().editing_other {
            self.page_mut().other_draft.push_str(text);
        } else if text == " " && self.question().multi_select {
            self.toggle_current();
        }
    }

    pub(super) fn backspace(&mut self) {
        if !self.is_submit_page() && self.page().editing_other {
            self.page_mut().other_draft.pop();
        }
    }

    /// Leaves the custom editor first; returns `true` only when the request should be dismissed.
    pub(super) fn escape(&mut self) -> bool {
        if !self.is_submit_page() && self.page().editing_other {
            self.page_mut().editing_other = false;
            false
        } else {
            true
        }
    }

    pub(super) fn confirm(&mut self) -> Option<QuestionResponse> {
        if self.is_submit_page() {
            return self
                .pages
                .iter()
                .all(|page| page.complete)
                .then(|| QuestionResponse {
                    request_id: self.request.id,
                    answers: self.answers(),
                });
        }
        if self.page().editing_other {
            if self.page().other_draft.trim().is_empty() {
                return None;
            }
            let multi = self.question().multi_select;
            let page = self.page_mut();
            page.other_selected = true;
            page.editing_other = false;
            page.complete = true;
            if !multi {
                page.selected.clear();
            }
        } else if self.cursor_is_other() {
            if self.page().other_draft.trim().is_empty() {
                self.page_mut().editing_other = true;
                return None;
            }
            let multi = self.question().multi_select;
            let page = self.page_mut();
            page.other_selected = true;
            page.complete = true;
            if !multi {
                page.selected.clear();
            }
        } else if self.question().multi_select {
            if self.page().selected.is_empty() && !self.page().other_selected {
                return None;
            }
            self.page_mut().complete = true;
        } else {
            let cursor = self.page().cursor;
            let page = self.page_mut();
            page.selected.clear();
            page.selected.insert(cursor);
            page.other_selected = false;
            page.complete = true;
        }
        self.finish_or_advance()
    }

    pub(super) fn inline_presentation(&self, visible_rows: usize) -> InlineQuestionPresentation {
        if self.is_submit_page() {
            return self.submit_presentation(visible_rows);
        }
        let question = self.question();
        let page = self.page();
        let mut rows = question
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| ListRowDisplay {
                number: index + 1,
                label: choice_label(
                    &option.label,
                    question.multi_select,
                    page.selected.contains(&index),
                ),
                context: None,
                description: Some(option.description.clone()).filter(|text| !text.is_empty()),
                selected: page.cursor == index,
                current: page.selected.contains(&index),
            })
            .collect::<Vec<_>>();
        let other_index = question.options.len();
        let other = if page.other_draft.is_empty() {
            "Other".to_owned()
        } else if page.editing_other {
            format!("Other: {}_", page.other_draft)
        } else {
            format!("Other: {}", page.other_draft)
        };
        rows.push(ListRowDisplay {
            number: other_index + 1,
            label: choice_label(&other, question.multi_select, page.other_selected),
            context: None,
            description: Some("Type a custom answer".to_owned()),
            selected: page.cursor == other_index,
            current: page.other_selected,
        });
        let start = page
            .cursor
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(rows.len().saturating_sub(visible_rows));
        let rows = rows.into_iter().skip(start).take(visible_rows).collect();
        InlineQuestionPresentation {
            title: question.question.clone(),
            rows,
            tabs: self.tabs(),
            help_hint: if page.editing_other {
                "enter save  esc options".to_owned()
            } else if question.multi_select {
                "space toggle  enter continue  esc dismiss".to_owned()
            } else {
                "enter choose  esc dismiss".to_owned()
            },
            submit_page: false,
            submit_ready: false,
        }
    }

    fn submit_presentation(&self, visible_rows: usize) -> InlineQuestionPresentation {
        let submit_ready = self.pages.iter().all(|page| page.complete);
        let reserved_rows = 1 + usize::from(!submit_ready);
        let rows = self
            .request
            .questions
            .iter()
            .zip(&self.pages)
            .enumerate()
            .map(|(index, (question, page))| ListRowDisplay {
                number: index + 1,
                label: if question.header.is_empty() {
                    format!("Q{}", index + 1)
                } else {
                    question.header.clone()
                },
                context: None,
                description: Some(if page.complete {
                    self.answer_for(question, page)
                } else {
                    "Not answered".to_owned()
                }),
                selected: false,
                current: page.complete,
            })
            .take(visible_rows.saturating_sub(reserved_rows))
            .collect();
        InlineQuestionPresentation {
            title: "Review your answers before submit".to_owned(),
            rows,
            tabs: self.tabs(),
            help_hint: "enter submit  ←/→ tabs  esc dismiss".to_owned(),
            submit_page: true,
            submit_ready,
        }
    }

    fn tabs(&self) -> Vec<(String, bool)> {
        let mut tabs = self
            .request
            .questions
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let label = if question.header.is_empty() {
                    format!("Q{}", index + 1)
                } else {
                    question.header.clone()
                };
                let label = if self.pages[index].complete {
                    format!("✓ {label}")
                } else {
                    label
                };
                (label, index == self.active)
            })
            .collect::<Vec<_>>();
        if self.has_submit_page() {
            tabs.push(("Submit".to_owned(), self.is_submit_page()));
        }
        tabs
    }

    fn toggle_current(&mut self) {
        if self.cursor_is_other() {
            let page = self.page_mut();
            page.other_selected = !page.other_selected;
            if page.other_selected {
                page.editing_other = true;
            }
            page.complete = false;
            return;
        }
        let cursor = self.page().cursor;
        let page = self.page_mut();
        if !page.selected.remove(&cursor) {
            page.selected.insert(cursor);
        }
        page.complete = false;
    }

    fn finish_or_advance(&mut self) -> Option<QuestionResponse> {
        if !self.has_submit_page() && self.pages.iter().all(|page| page.complete) {
            return Some(QuestionResponse {
                request_id: self.request.id,
                answers: self.answers(),
            });
        }
        self.active = (self.active + 1).min(self.pages.len());
        None
    }

    fn answers(&self) -> BTreeMap<String, String> {
        self.request
            .questions
            .iter()
            .zip(&self.pages)
            .map(|(question, page)| (question.question.clone(), self.answer_for(question, page)))
            .collect()
    }

    fn answer_for(&self, question: &misy_core::QuestionItem, page: &QuestionPage) -> String {
        let mut values = question
            .options
            .iter()
            .enumerate()
            .filter(|(index, _)| page.selected.contains(index))
            .map(|(_, option)| option.label.clone())
            .collect::<Vec<_>>();
        if page.other_selected {
            values.push(page.other_draft.clone());
        }
        values.join(", ")
    }

    fn has_submit_page(&self) -> bool {
        self.pages.len() > 1
    }

    fn is_submit_page(&self) -> bool {
        self.has_submit_page() && self.active == self.pages.len()
    }

    fn tab_count(&self) -> usize {
        self.pages.len() + usize::from(self.has_submit_page())
    }

    fn question(&self) -> &misy_core::QuestionItem {
        &self.request.questions[self.active]
    }

    fn page(&self) -> &QuestionPage {
        &self.pages[self.active]
    }

    fn page_mut(&mut self) -> &mut QuestionPage {
        &mut self.pages[self.active]
    }

    fn row_count(&self) -> usize {
        self.question().options.len() + 1
    }

    fn cursor_is_other(&self) -> bool {
        self.page().cursor == self.question().options.len()
    }
}

fn choice_label(label: &str, multi: bool, selected: bool) -> String {
    if multi {
        format!("[{}] {label}", if selected { "x" } else { " " })
    } else {
        label.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::QuestionDialog;
    use misy_core::QuestionRequest;
    use serde_json::json;

    fn request(multi_select: bool) -> QuestionRequest {
        serde_json::from_value(json!({
            "id": 1,
            "tool_call_id": "call-1",
            "source": {"Submission": 1},
            "questions": [{
                "question": "Choose",
                "header": "",
                "options": [
                    {"label": "A", "description": ""},
                    {"label": "B", "description": ""}
                ],
                "multi_select": multi_select
            }]
        }))
        .expect("test question request")
    }

    fn two_question_request() -> QuestionRequest {
        serde_json::from_value(json!({
            "id": 1,
            "tool_call_id": "call-1",
            "source": {"Submission": 1},
            "questions": [
                {
                    "question": "First?",
                    "header": "First",
                    "options": [{"label": "A"}, {"label": "B"}]
                },
                {
                    "question": "Second?",
                    "header": "Second",
                    "options": [{"label": "C"}, {"label": "D"}]
                }
            ]
        }))
        .expect("multi-question request")
    }

    #[test]
    fn single_select_returns_exact_label() {
        let mut dialog = QuestionDialog::new(request(false));
        dialog.move_down();
        let response = dialog
            .confirm()
            .expect("one question completes immediately");
        assert_eq!(response.answers["Choose"], "B");
    }

    #[test]
    fn multiple_questions_require_the_separate_submit_tab() {
        let mut dialog = QuestionDialog::new(two_question_request());

        assert!(dialog.confirm().is_none());
        let second = dialog.inline_presentation(8);
        assert_eq!(second.title, "Second?");
        assert!(second.tabs.iter().any(|(label, _)| label == "Submit"));

        dialog.move_down();
        assert!(dialog.confirm().is_none());
        let review = dialog.inline_presentation(8);
        assert!(review.submit_page);
        assert!(review.submit_ready);
        assert_eq!(review.rows[0].description.as_deref(), Some("A"));
        assert_eq!(review.rows[1].description.as_deref(), Some("D"));

        let response = dialog.confirm().expect("Submit tab confirms all answers");
        assert_eq!(response.answers["First?"], "A");
        assert_eq!(response.answers["Second?"], "D");
    }

    #[test]
    fn submit_tab_does_not_send_incomplete_answers() {
        let mut dialog = QuestionDialog::new(two_question_request());
        dialog.tab_left();

        let review = dialog.inline_presentation(8);
        assert!(review.submit_page);
        assert!(!review.submit_ready);
        assert_eq!(review.rows[0].description.as_deref(), Some("Not answered"));
        assert!(dialog.confirm().is_none());
    }

    #[test]
    fn other_editor_requires_nonempty_text() {
        let mut dialog = QuestionDialog::new(request(false));
        dialog.move_up();
        assert!(dialog.confirm().is_none());
        assert!(dialog.editing_other());
        assert!(dialog.confirm().is_none());
        dialog.insert_text("Custom");
        let response = dialog.confirm().expect("custom answer completes request");
        assert_eq!(response.answers["Choose"], "Custom");
    }

    #[test]
    fn custom_only_multi_select_submits_from_other_row() {
        let mut dialog = QuestionDialog::new(request(true));
        dialog.move_up();
        dialog.insert_text(" ");
        assert!(dialog.editing_other());
        dialog.insert_text("Custom");
        let response = dialog.confirm().expect("custom answer completes request");
        assert_eq!(response.answers["Choose"], "Custom");
    }

    #[test]
    fn constrained_viewport_keeps_the_cursor_visible() {
        let mut request = request(false);
        request.questions[0]
            .options
            .push(misy_core::QuestionOption {
                label: "C".to_owned(),
                description: String::new(),
            });
        request.questions[0]
            .options
            .push(misy_core::QuestionOption {
                label: "D".to_owned(),
                description: String::new(),
            });
        let mut dialog = QuestionDialog::new(request);
        dialog.move_up();

        let rows = dialog.inline_presentation(2).rows;
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.selected && row.label.contains("Other"))
        );
    }
}
