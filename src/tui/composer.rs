//! Editable prompt state and slash-command completion.

use ratatui::text::Line;

const MAX_HISTORY_ENTRIES: usize = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommandDefinition {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
}

/// One visible slash-command completion row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommandPopupRow {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) selected: bool,
}

pub(super) const COMMANDS: [CommandDefinition; 3] = [
    CommandDefinition {
        name: "/provider",
        description: "Configure provider authentication",
    },
    CommandDefinition {
        name: "/model",
        description: "Choose a model",
    },
    CommandDefinition {
        name: "/usage",
        description: "Show provider usage and limits",
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommandPopup {
    selected: usize,
    dismissed_token: Option<String>,
}

impl CommandPopup {
    fn new() -> Self {
        Self {
            selected: 0,
            dismissed_token: None,
        }
    }

    fn matches(&self, text: &str) -> Vec<&'static CommandDefinition> {
        let filter = text.strip_prefix('/').unwrap_or_default();
        COMMANDS
            .iter()
            .filter(|command| command.name[1..].starts_with(filter))
            .collect()
    }
}

/// Text buffer retained while transient bottom-pane views are active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Composer {
    text: String,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
    popup: CommandPopup,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            popup: CommandPopup::new(),
        }
    }
}

impl Composer {
    pub(super) fn with_history(history: Vec<String>) -> Self {
        let mut composer = Self::default();
        for entry in history {
            composer.record_submitted(&entry);
        }
        composer
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn history_len(&self) -> usize {
        self.history.len()
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        self.detach_history();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.sync_popup_after_edit();
    }

    pub(super) fn insert_newline(&mut self) {
        self.insert_str("\n");
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = previous_boundary(&self.text, self.cursor);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = next_boundary(&self.text, self.cursor);
    }

    pub(super) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.detach_history();
        let start = previous_boundary(&self.text, self.cursor);
        self.text.drain(start..self.cursor);
        self.cursor = start;
        self.sync_popup_after_edit();
    }

    pub(super) fn delete(&mut self) {
        if self.cursor == self.text.len() {
            return;
        }
        self.detach_history();
        let end = next_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..end);
        self.sync_popup_after_edit();
    }

    pub(super) fn popup_visible(&self) -> bool {
        let Some(token) = command_token(&self.text, self.cursor) else {
            return false;
        };
        self.popup.dismissed_token.as_deref() != Some(token)
    }

    pub(super) fn popup_rows(&self) -> Vec<String> {
        self.popup_rows_for_render()
            .into_iter()
            .map(|row| {
                let marker = if row.selected { "›" } else { " " };
                format!("{marker} {}  {}", row.name, row.description)
            })
            .collect()
    }

    pub(super) fn popup_rows_for_render(&self) -> Vec<CommandPopupRow> {
        if !self.popup_visible() {
            return Vec::new();
        }
        self.popup
            .matches(&self.text)
            .iter()
            .enumerate()
            .map(|(index, command)| CommandPopupRow {
                name: command.name,
                description: command.description,
                selected: index == self.popup.selected,
            })
            .collect()
    }

    pub(super) fn popup_up(&mut self) {
        let count = self.popup.matches(&self.text).len();
        if count != 0 {
            self.popup.selected = (self.popup.selected + count - 1) % count;
        }
    }

    pub(super) fn popup_down(&mut self) {
        let count = self.popup.matches(&self.text).len();
        if count != 0 {
            self.popup.selected = (self.popup.selected + 1) % count;
        }
    }

    pub(super) fn dismiss_popup(&mut self) {
        self.popup.dismissed_token = command_token(&self.text, self.cursor).map(str::to_owned);
    }

    pub(super) fn selected_command(&self) -> Option<&'static str> {
        self.popup
            .matches(&self.text)
            .get(self.popup.selected)
            .map(|command| command.name)
    }

    pub(super) fn take_text(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.history_index = None;
        self.history_draft = None;
        self.popup = CommandPopup::new();
        text
    }

    pub(super) fn record_submitted(&mut self, text: &str) -> bool {
        if text.trim().is_empty() || self.history.last().map(String::as_str) == Some(text) {
            return false;
        }
        self.history.push(text.to_owned());
        if self.history.len() > MAX_HISTORY_ENTRIES {
            self.history.remove(0);
        }
        self.history_index = None;
        self.history_draft = None;
        true
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_index = None;
        self.history_draft = None;
        self.popup = CommandPopup::new();
    }

    pub(super) fn history_previous(&mut self) {
        if (self.cursor != 0 && self.cursor != self.text.len()) || self.history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft = Some(self.text.clone());
                self.history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.text.clone_from(&self.history[index]);
        self.cursor = self.text.len();
        self.dismiss_recalled_command();
    }

    pub(super) fn history_next(&mut self) {
        if self.cursor != self.text.len() {
            return;
        }
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.history_index = Some(next);
            self.text.clone_from(&self.history[next]);
        } else {
            self.text = self.history_draft.take().unwrap_or_default();
            self.history_index = None;
        }
        self.cursor = self.text.len();
        self.dismiss_recalled_command();
    }

    pub(super) fn cursor_position(&self) -> (u16, u16) {
        let prefix = &self.text[..self.cursor];
        let row = prefix
            .chars()
            .filter(|character| *character == '\n')
            .count();
        let column_text = prefix.rsplit('\n').next().unwrap_or_default();
        (
            u16::try_from(Line::from(column_text).width()).unwrap_or(u16::MAX),
            u16::try_from(row).unwrap_or(u16::MAX),
        )
    }

    pub(super) fn position_cursor(&mut self, row: u16, column: u16) {
        let target_row = usize::from(row);
        let mut offset = 0;
        let line = self.text.split('\n').nth(target_row);
        let Some(line) = line else {
            self.cursor = self.text.len();
            return;
        };
        for preceding in self.text.split('\n').take(target_row) {
            offset += preceding.len() + 1;
        }
        self.cursor = offset + byte_at_display_column(line, usize::from(column));
        self.detach_history();
        self.sync_popup_after_edit();
    }

    fn detach_history(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn sync_popup_after_edit(&mut self) {
        let token = command_token(&self.text, self.cursor);
        if self.popup.dismissed_token.as_deref() != token {
            self.popup.dismissed_token = None;
        }
        let count = self.popup.matches(&self.text).len();
        self.popup.selected = self.popup.selected.min(count.saturating_sub(1));
    }

    fn dismiss_recalled_command(&mut self) {
        self.popup.selected = 0;
        self.popup.dismissed_token = command_token(&self.text, self.cursor).map(str::to_owned);
    }
}

fn command_token(text: &str, cursor: usize) -> Option<&str> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let first_line_end = text.find('\n').unwrap_or(text.len());
    if cursor > first_line_end || !text.starts_with('/') {
        return None;
    }
    let first_line = &text[..first_line_end];
    if first_line.chars().any(char::is_whitespace) {
        return None;
    }
    Some(first_line)
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}

fn byte_at_display_column(line: &str, column: usize) -> usize {
    let mut display_column: usize = 0;
    for (byte, character) in line.char_indices() {
        let width = Line::from(character.to_string()).width();
        if display_column.saturating_add(width) > column {
            return byte;
        }
        display_column = display_column.saturating_add(width);
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::Composer;

    #[test]
    fn edits_unicode_at_the_cursor() {
        let mut composer = Composer::default();
        composer.insert_str("aйc");
        composer.move_left();
        composer.backspace();
        composer.insert_str("b");
        assert_eq!(composer.text(), "abc");
        composer.move_home();
        composer.delete();
        assert_eq!(composer.text(), "bc");
    }

    #[test]
    fn slash_popup_tracks_edits_and_dismissal() {
        let mut composer = Composer::default();
        composer.insert_str("/");
        assert_eq!(composer.popup_rows().len(), 3);
        composer.insert_str("mo");
        assert_eq!(composer.selected_command(), Some("/model"));
        composer.dismiss_popup();
        assert!(!composer.popup_visible());
        composer.insert_str("d");
        assert!(composer.popup_visible());
    }

    #[test]
    fn seeded_history_is_bounded_and_collapses_adjacent_duplicates() {
        let history = (0..105)
            .map(|index| format!("prompt-{index}"))
            .chain(["prompt-104".to_owned()])
            .collect();
        let mut composer = Composer::with_history(history);

        assert_eq!(composer.history_len(), 100);
        composer.history_previous();
        assert_eq!(composer.text(), "prompt-104");
    }

    #[test]
    fn mouse_position_maps_display_cells_to_unicode_boundaries() {
        let mut composer = Composer::default();
        composer.insert_str("a界b\nnext");

        composer.position_cursor(0, 2);
        composer.insert_str("!");
        assert_eq!(composer.text(), "a!界b\nnext");

        composer.position_cursor(1, 2);
        composer.insert_str("!");
        assert_eq!(composer.text(), "a!界b\nne!xt");

        composer.position_cursor(9, 0);
        composer.insert_str("!");
        assert_eq!(composer.text(), "a!界b\nne!xt!");
    }
}
