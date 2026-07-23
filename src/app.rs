#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Sections,
    Messages,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            messages: (1..=36).map(|n| format!("Archived message {n}")).collect(),
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
                    FocusedPane::Messages => FocusedPane::Sections,
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
        }
    }
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
    fn quit_action_stops_the_app() {
        let mut state = AppState::new(fixture_sections());
        let outcome = state.apply(Action::Quit);
        assert_eq!(outcome, Outcome::Quit);
        assert!(!state.running);
    }
}
