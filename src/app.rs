#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Sections,
    Messages,
    Composer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub id: u64,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerState {
    text: String,
    cursor: usize,
    scroll: usize,
    attachments: Vec<ImageAttachment>,
    next_attachment_id: u64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    MoveUp,
    MoveDown,
    MoveUpIn(FocusedPane),
    MoveDownIn(FocusedPane),
    FocusLeft,
    FocusRight,
    CycleFocus,
    Activate,
    SelectSection(usize),
    SelectMessage(usize),
    InsertText(String),
    InsertLineBreak,
    Backspace,
    Delete,
    MoveCursorLeft,
    MoveCursorRight,
    ScrollComposerUp,
    ScrollComposerDown,
    InsertImage {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    SubmitComposer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub sections: Vec<Section>,
    pub selected_section: usize,
    pub selected_message: usize,
    pub focused_pane: FocusedPane,
    pub running: bool,
    composer: ComposerState,
}

pub fn fixture_sections() -> Vec<Section> {
    vec![
        Section {
            title: "Inbox".into(),
            messages: vec![],
        },
        Section {
            title: "Today".into(),
            messages: vec!["Boot complete".into()],
        },
        Section {
            title: "Work".into(),
            messages: vec![
                "Draft plan".into(),
                "Review state".into(),
                "Ship fix".into(),
                "Write tests".into(),
                "Verify smoke".into(),
            ],
        },
        Section {
            title: "Archive".into(),
            messages: (1..=132).map(|n| format!("Archived message {n}")).collect(),
        },
    ]
}

impl AppState {
    pub fn new(sections: Vec<Section>) -> Self {
        Self {
            sections,
            selected_section: 0,
            selected_message: 0,
            focused_pane: FocusedPane::Sections,
            running: true,
            composer: ComposerState {
                text: String::new(),
                cursor: 0,
                scroll: 0,
                attachments: Vec::new(),
                next_attachment_id: 0,
            },
        }
    }

    pub fn selected_section(&self) -> Option<&Section> {
        self.sections.get(self.selected_section)
    }

    pub fn current_messages(&self) -> &[String] {
        self.selected_section()
            .map(|section| section.messages.as_slice())
            .unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub fn composer_text(&self) -> &str {
        &self.composer.text
    }

    #[allow(dead_code)]
    pub fn composer_cursor(&self) -> usize {
        self.composer.cursor
    }

    #[allow(dead_code)]
    pub fn composer_scroll(&self) -> usize {
        self.composer.scroll
    }

    #[allow(dead_code)]
    pub fn set_composer_scroll(&mut self, scroll: usize) {
        self.composer.scroll = scroll;
    }

    #[allow(dead_code)]
    pub fn attachments(&self) -> &[ImageAttachment] {
        &self.composer.attachments
    }

    pub fn apply(&mut self, action: Action) -> Outcome {
        match action {
            Action::Quit => {
                self.running = false;
                Outcome::Quit
            }
            Action::MoveUp => {
                self.move_selection_up_in(self.focused_pane);
                Outcome::Continue
            }
            Action::MoveDown => {
                self.move_selection_down_in(self.focused_pane);
                Outcome::Continue
            }
            Action::MoveUpIn(pane) => {
                self.move_selection_up_in(pane);
                Outcome::Continue
            }
            Action::MoveDownIn(pane) => {
                self.move_selection_down_in(pane);
                Outcome::Continue
            }
            Action::FocusLeft => {
                self.focused_pane = FocusedPane::Sections;
                Outcome::Continue
            }
            Action::FocusRight => {
                self.focused_pane = FocusedPane::Messages;
                Outcome::Continue
            }
            Action::CycleFocus => {
                self.focused_pane = match self.focused_pane {
                    FocusedPane::Sections => FocusedPane::Messages,
                    FocusedPane::Messages => FocusedPane::Composer,
                    FocusedPane::Composer => FocusedPane::Sections,
                };
                Outcome::Continue
            }
            Action::Activate => {
                if self.focused_pane == FocusedPane::Sections {
                    self.focused_pane = FocusedPane::Messages;
                }
                Outcome::Continue
            }
            Action::SelectSection(index) => {
                self.select_section(index);
                Outcome::Continue
            }
            Action::SelectMessage(index) => {
                self.select_message(index);
                Outcome::Continue
            }
            Action::InsertText(text) => {
                self.insert_text_at_cursor(&text);
                Outcome::Continue
            }
            Action::InsertLineBreak => {
                self.insert_text_at_cursor("\n");
                Outcome::Continue
            }
            Action::Backspace => {
                self.backspace_composer();
                Outcome::Continue
            }
            Action::Delete => {
                self.delete_composer();
                Outcome::Continue
            }
            Action::MoveCursorLeft => {
                let cursor = previous_scalar_boundary(&self.composer.text, self.composer.cursor);
                self.composer.cursor = image_token_range_at(&self.composer.text, cursor)
                    .map_or(cursor, |(start, _, _)| start);
                Outcome::Continue
            }
            Action::MoveCursorRight => {
                let cursor = next_scalar_boundary(&self.composer.text, self.composer.cursor);
                self.composer.cursor = image_token_range_at(&self.composer.text, cursor)
                    .map_or(cursor, |(_, end, _)| end);
                Outcome::Continue
            }
            Action::ScrollComposerUp => {
                self.composer.scroll = self.composer.scroll.saturating_sub(1);
                Outcome::Continue
            }
            Action::ScrollComposerDown => {
                self.composer.scroll = self.composer.scroll.saturating_add(1);
                Outcome::Continue
            }
            Action::InsertImage {
                width,
                height,
                rgba,
            } => {
                let id = self.composer.next_attachment_id;
                self.composer.next_attachment_id =
                    self.composer.next_attachment_id.saturating_add(1);
                self.composer.attachments.push(ImageAttachment {
                    id,
                    width,
                    height,
                    rgba,
                });
                let token = image_token(id);
                self.insert_text_at_cursor(&token);
                Outcome::Continue
            }
            Action::SubmitComposer => {
                self.composer.text.clear();
                self.composer.cursor = 0;
                self.composer.scroll = 0;
                self.composer.attachments.clear();
                Outcome::Continue
            }
        }
    }

    fn select_section(&mut self, index: usize) {
        if self.sections.is_empty() {
            self.selected_section = 0;
            self.selected_message = 0;
            self.focused_pane = FocusedPane::Sections;
            return;
        }

        self.selected_section = index.min(self.sections.len() - 1);
        self.selected_message = 0;
        self.focused_pane = FocusedPane::Sections;
    }

    fn select_message(&mut self, index: usize) {
        let Some(section) = self.selected_section() else {
            self.selected_message = 0;
            self.focused_pane = FocusedPane::Messages;
            return;
        };

        if section.messages.is_empty() {
            self.selected_message = 0;
        } else {
            self.selected_message = index.min(section.messages.len() - 1);
        }
        self.focused_pane = FocusedPane::Messages;
    }

    fn move_selection_up_in(&mut self, pane: FocusedPane) {
        match pane {
            FocusedPane::Sections => {
                if self.sections.is_empty() {
                    return;
                }
                self.selected_section = self.selected_section.saturating_sub(1);
                self.selected_message = 0;
            }
            FocusedPane::Messages => {
                self.selected_message = self.selected_message.saturating_sub(1);
            }
            FocusedPane::Composer => {}
        }
    }

    fn move_selection_down_in(&mut self, pane: FocusedPane) {
        match pane {
            FocusedPane::Sections => {
                if self.sections.is_empty() {
                    return;
                }
                let last = self.sections.len() - 1;
                self.selected_section = self.selected_section.min(last).saturating_add(1).min(last);
                self.selected_message = 0;
            }
            FocusedPane::Messages => {
                let Some(section) = self.selected_section() else {
                    self.selected_message = 0;
                    return;
                };
                if section.messages.is_empty() {
                    self.selected_message = 0;
                } else {
                    let last = section.messages.len() - 1;
                    self.selected_message =
                        self.selected_message.min(last).saturating_add(1).min(last);
                }
            }
            FocusedPane::Composer => {}
        }
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        self.remove_image_token_at_cursor();
        self.composer.text.insert_str(self.composer.cursor, text);
        self.composer.cursor += text.len();
    }

    fn backspace_composer(&mut self) {
        if self.remove_image_token_at_cursor() {
            return;
        }

        if let Some((start, end, id)) =
            image_token_ending_at(&self.composer.text, self.composer.cursor)
        {
            self.remove_image_token(start, end, id);
            return;
        }

        let start = previous_scalar_boundary(&self.composer.text, self.composer.cursor);
        if start == self.composer.cursor {
            return;
        }

        self.composer
            .text
            .replace_range(start..self.composer.cursor, "");
        self.composer.cursor = start;
    }

    fn delete_composer(&mut self) {
        if self.remove_image_token_at_cursor() {
            return;
        }

        if let Some((start, end, id)) =
            image_token_starting_at(&self.composer.text, self.composer.cursor)
        {
            self.remove_image_token(start, end, id);
            return;
        }

        let end = next_scalar_boundary(&self.composer.text, self.composer.cursor);
        if end == self.composer.cursor {
            return;
        }

        self.composer
            .text
            .replace_range(self.composer.cursor..end, "");
    }

    fn remove_image_token_at_cursor(&mut self) -> bool {
        let Some((start, end, id)) =
            image_token_range_at(&self.composer.text, self.composer.cursor)
        else {
            return false;
        };
        self.remove_image_token(start, end, id);
        true
    }

    fn remove_image_token(&mut self, start: usize, end: usize, id: u64) {
        self.composer.text.replace_range(start..end, "");
        self.composer.cursor = start;
        self.composer
            .attachments
            .retain(|attachment| attachment.id != id);
    }
}

fn previous_scalar_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }

    text[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_scalar_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    let mut chars = text[cursor..].chars();
    let Some(ch) = chars.next() else {
        return text.len();
    };
    cursor + ch.len_utf8()
}

fn image_token(id: u64) -> String {
    format!("[Image #{id}]")
}

fn image_token_ending_at(text: &str, cursor: usize) -> Option<(usize, usize, u64)> {
    let prefix = &text[..cursor];
    let start = prefix.rfind("[Image #")?;
    parse_image_token_at(text, start).filter(|(_, end, _)| *end == cursor)
}

fn image_token_starting_at(text: &str, cursor: usize) -> Option<(usize, usize, u64)> {
    parse_image_token_at(text, cursor)
}

fn image_token_range_at(text: &str, cursor: usize) -> Option<(usize, usize, u64)> {
    let start = text[..cursor].rfind('[')?;
    parse_image_token_at(text, start)
        .filter(|(token_start, token_end, _)| *token_start < cursor && cursor < *token_end)
}

fn parse_image_token_at(text: &str, start: usize) -> Option<(usize, usize, u64)> {
    let rest = text.get(start..)?;
    let digits = rest.strip_prefix("[Image #")?;
    let close = digits.find(']')?;
    let number = digits.get(..close)?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let end = start + "[Image #".len() + close + 1;
    let id = number.parse::<u64>().ok()?;
    Some((start, end, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_sections_cover_empty_short_medium_and_long_cases() {
        let sections = fixture_sections();
        assert_eq!(sections.len(), 4);
        assert!(sections.iter().any(|section| section.messages.is_empty()));
        assert!(sections.iter().any(|section| section.messages.len() == 1));
        assert!(sections.iter().any(|section| section.messages.len() >= 5));
        assert!(sections.iter().any(|section| section.messages.len() >= 30));
    }

    #[test]
    fn new_state_starts_with_sections_focus_and_first_items_selected() {
        let state = AppState::new(fixture_sections());
        assert!(state.running);
        assert_eq!(state.focused_pane, FocusedPane::Sections);
        assert_eq!(state.selected_section, 0);
        assert_eq!(state.selected_message, 0);
    }

    #[test]
    fn empty_sections_keep_selection_and_focus_actions_safe() {
        let mut state = AppState::new(vec![]);
        assert_eq!(state.apply(Action::MoveUp), Outcome::Continue);
        assert_eq!(state.apply(Action::MoveDown), Outcome::Continue);
        assert_eq!(state.apply(Action::FocusRight), Outcome::Continue);
        assert_eq!(state.selected_section, 0);
        assert_eq!(state.selected_message, 0);
    }

    #[test]
    fn moving_down_in_sections_changes_section_and_resets_message_selection() {
        let mut state = AppState::new(fixture_sections());
        state.selected_message = 3;

        let outcome = state.apply(Action::MoveDown);

        assert_eq!(outcome, Outcome::Continue);
        assert_eq!(state.selected_section, 1);
        assert_eq!(state.selected_message, 0);
    }

    #[test]
    fn message_navigation_clamps_at_list_boundaries() {
        let mut state = AppState::new(fixture_sections());
        state.apply(Action::SelectSection(2));
        state.apply(Action::FocusRight);
        state.apply(Action::MoveDown);
        state.apply(Action::MoveDown);
        state.apply(Action::MoveDown);
        state.apply(Action::MoveDown);
        assert_eq!(state.selected_message, 4);

        state.apply(Action::MoveUp);
        state.apply(Action::MoveUp);
        state.apply(Action::MoveUp);
        state.apply(Action::MoveUp);
        state.apply(Action::MoveUp);
        state.apply(Action::MoveUp);
        assert_eq!(state.selected_message, 0);
    }

    #[test]
    fn activate_moves_focus_only_from_sections_to_messages() {
        let mut state = AppState::new(fixture_sections());
        assert_eq!(state.apply(Action::Activate), Outcome::Continue);
        assert_eq!(state.focused_pane, FocusedPane::Messages);
        assert_eq!(state.apply(Action::Activate), Outcome::Continue);
        assert_eq!(state.focused_pane, FocusedPane::Messages);
    }

    #[test]
    fn composer_submit_clears_multiline_draft_scroll_and_attachments() {
        let mut state = AppState::new(fixture_sections());
        let sections_before = state.sections.clone();

        state.apply(Action::InsertText("hello\nworld".into()));
        state.apply(Action::InsertImage {
            width: 2,
            height: 1,
            rgba: vec![0, 1, 2, 3, 4, 5, 6, 7],
        });
        state.set_composer_scroll(3);

        state.apply(Action::SubmitComposer);

        assert_eq!(state.composer_text(), "");
        assert_eq!(state.composer_cursor(), 0);
        assert_eq!(state.composer_scroll(), 0);
        assert!(state.attachments().is_empty());
        assert_eq!(state.sections, sections_before);
    }

    #[test]
    fn composer_backspace_removes_image_token_and_attachment_atomically() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![9, 8, 7, 6],
        });
        assert_eq!(state.composer_text(), "[Image #0]");

        state.apply(Action::Backspace);

        assert_eq!(state.composer_text(), "");
        assert_eq!(state.composer_cursor(), 0);
        assert!(state.attachments().is_empty());
    }

    #[test]
    fn composer_backspace_keeps_first_attachment_id_after_second_token_is_removed() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
        });
        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![1, 1, 1, 1],
        });

        state.apply(Action::Backspace);

        assert_eq!(state.composer_text(), "[Image #0]");
        assert_eq!(state.attachments().len(), 1);
        assert_eq!(state.attachments()[0].id, 0);
    }

    #[test]
    fn composer_cursor_moves_skip_over_image_tokens() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertText("a".into()));
        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![7, 7, 7, 7],
        });
        state.apply(Action::InsertText("b".into()));

        state.composer.cursor = 1;
        state.apply(Action::MoveCursorRight);
        assert_eq!(state.composer_cursor(), 11);

        state.apply(Action::MoveCursorLeft);
        assert_eq!(state.composer_cursor(), 1);
    }

    #[test]
    fn composer_insert_text_from_image_token_middle_removes_token_and_attachment() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertText("a".into()));
        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![5, 5, 5, 5],
        });
        state.apply(Action::InsertText("b".into()));
        state.composer.cursor = 4;

        state.apply(Action::InsertText("x".into()));

        assert_eq!(state.composer_text(), "axb");
        assert_eq!(state.composer_cursor(), 2);
        assert!(state.attachments().is_empty());
    }

    #[test]
    fn composer_backspace_from_image_token_middle_removes_token_and_attachment() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertText("a".into()));
        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![4, 4, 4, 4],
        });
        state.apply(Action::InsertText("b".into()));
        state.composer.cursor = 4;

        state.apply(Action::Backspace);

        assert_eq!(state.composer_text(), "ab");
        assert_eq!(state.composer_cursor(), 1);
        assert!(state.attachments().is_empty());
    }

    #[test]
    fn composer_delete_from_image_token_middle_removes_token_and_attachment() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertText("a".into()));
        state.apply(Action::InsertImage {
            width: 1,
            height: 1,
            rgba: vec![3, 3, 3, 3],
        });
        state.apply(Action::InsertText("b".into()));
        state.composer.cursor = 4;

        state.apply(Action::Delete);

        assert_eq!(state.composer_text(), "ab");
        assert_eq!(state.composer_cursor(), 1);
        assert!(state.attachments().is_empty());
    }

    #[test]
    fn composer_utf8_cursor_left_then_backspace_removes_single_scalar() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::InsertText("é🙂".into()));
        state.apply(Action::MoveCursorLeft);
        state.apply(Action::Backspace);

        assert_eq!(state.composer_text(), "🙂");
        assert_eq!(state.composer_cursor(), 0);
    }

    #[test]
    fn composer_scroll_actions_clamp_at_zero_for_empty_draft() {
        let mut state = AppState::new(fixture_sections());

        state.apply(Action::ScrollComposerDown);
        state.apply(Action::ScrollComposerUp);

        assert_eq!(state.composer_scroll(), 0);
    }

    #[test]
    fn quit_action_stops_the_app() {
        let mut state = AppState::new(fixture_sections());
        let outcome = state.apply(Action::Quit);
        assert_eq!(outcome, Outcome::Quit);
        assert!(!state.running);
    }
}
