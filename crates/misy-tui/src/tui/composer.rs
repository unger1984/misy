//! Editable prompt state and slash-command completion.

use super::composer_attachment::{ComposerAttachments, ComposerDraft};
use misy_core::ImageAttachment;
use ratatui::text::Line;

const MAX_HISTORY_ENTRIES: usize = 100;
const MAX_POPUP_ROWS: usize = 8;

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

pub(super) const COMMANDS: [CommandDefinition; 9] = [
    CommandDefinition {
        name: "/provider",
        description: "Configure provider authentication",
    },
    CommandDefinition {
        name: "/model",
        description: "Choose a model",
    },
    CommandDefinition {
        name: "/tasks",
        description: "Show background tasks and agents",
    },
    CommandDefinition {
        name: "/new",
        description: "Start a new conversation",
    },
    CommandDefinition {
        name: "/clear",
        description: "Start a new conversation",
    },
    CommandDefinition {
        name: "/resume",
        description: "Resume a saved conversation",
    },
    CommandDefinition {
        name: "/status",
        description: "Show provider usage and limits",
    },
    CommandDefinition {
        name: "/usage",
        description: "Show provider usage and limits",
    },
    CommandDefinition {
        name: "/exit",
        description: "Exit Misy",
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
    attachments: ComposerAttachments,
    history: Vec<ComposerSnapshot>,
    history_index: Option<usize>,
    history_draft: Option<ComposerSnapshot>,
    popup: CommandPopup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ComposerSnapshot {
    text: String,
    cursor: usize,
    attachments: ComposerAttachments,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            attachments: ComposerAttachments::default(),
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

    pub(super) fn attachment_count(&self) -> usize {
        self.attachments.len()
    }

    pub(super) fn draft(&self) -> ComposerDraft {
        self.attachments.draft(&self.text)
    }

    pub(super) fn history_text(&self) -> String {
        self.attachments.history_text(&self.text)
    }

    pub(super) fn history_snapshot(&self) -> ComposerSnapshot {
        self.snapshot()
    }

    pub(super) fn matches_draft(&self, draft: &ComposerDraft) -> bool {
        self.draft() == *draft
    }

    pub(super) fn insert_image(&mut self, image: ImageAttachment) -> Result<(), &'static str> {
        self.detach_history();
        self.attachments
            .insert(&mut self.text, &mut self.cursor, image)?;
        self.sync_popup_after_edit();
        Ok(())
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        self.detach_history();
        self.attachments.shift_for_insert(self.cursor, text.len());
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.sync_popup_after_edit();
    }

    pub(super) fn insert_newline(&mut self) {
        self.insert_str("\n");
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = self.attachments.previous_cursor(&self.text, self.cursor);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = self.attachments.next_cursor(&self.text, self.cursor);
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
        if self
            .attachments
            .remove_before(&mut self.text, &mut self.cursor)
        {
            self.sync_popup_after_edit();
            return;
        }
        let start = previous_boundary(&self.text, self.cursor);
        self.text.drain(start..self.cursor);
        self.attachments.shift_for_remove(start..self.cursor);
        self.cursor = start;
        self.sync_popup_after_edit();
    }

    pub(super) fn delete(&mut self) {
        if self.cursor == self.text.len() {
            return;
        }
        self.detach_history();
        if self.attachments.remove_at(&mut self.text, &mut self.cursor) {
            self.sync_popup_after_edit();
            return;
        }
        let end = next_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..end);
        self.attachments.shift_for_remove(self.cursor..end);
        self.sync_popup_after_edit();
    }

    pub(super) fn popup_visible(&self) -> bool {
        if self.attachments.len() != 0 {
            return false;
        }
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
        let matches = self.popup.matches(&self.text);
        let scroll_top = self
            .popup
            .selected
            .saturating_add(1)
            .saturating_sub(MAX_POPUP_ROWS);
        matches
            .iter()
            .enumerate()
            .skip(scroll_top)
            .take(MAX_POPUP_ROWS)
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

    pub(super) fn complete_selected_command(&mut self) -> bool {
        let Some(command) = self.selected_command() else {
            return false;
        };
        let first_line_end = self.text.find('\n').unwrap_or(self.text.len());
        let completion = format!("{command} ");
        self.text.replace_range(..first_line_end, &completion);
        self.cursor = completion.len();
        self.detach_history();
        self.popup = CommandPopup::new();
        true
    }

    pub(super) fn record_submitted(&mut self, text: &str) -> bool {
        self.record_submitted_snapshot(ComposerSnapshot {
            text: text.to_owned(),
            cursor: text.len(),
            attachments: ComposerAttachments::default(),
        })
    }

    pub(super) fn record_submitted_snapshot(&mut self, snapshot: ComposerSnapshot) -> bool {
        if (snapshot.text.trim().is_empty() && snapshot.attachments.len() == 0)
            || self
                .history
                .last()
                .is_some_and(|entry| entry.matches_entry(&snapshot))
        {
            return false;
        }
        if snapshot.attachments.len() != 0 {
            for entry in &mut self.history {
                entry.remove_attachments();
            }
        }
        self.history.push(snapshot);
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
        self.attachments.clear();
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
                self.history_draft = Some(self.snapshot());
                self.history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.restore_snapshot(self.history[index].clone());
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
            self.restore_snapshot(self.history[next].clone());
        } else {
            self.restore_history_draft();
            self.history_index = None;
        }
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
        let cursor = offset + byte_at_display_column(line, usize::from(column));
        self.cursor = self.attachments.snap_cursor(cursor);
        self.detach_history();
        self.sync_popup_after_edit();
    }

    fn detach_history(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn snapshot(&self) -> ComposerSnapshot {
        ComposerSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            attachments: self.attachments.clone(),
        }
    }

    fn restore_history_draft(&mut self) {
        let Some(snapshot) = self.history_draft.take() else {
            self.text.clear();
            self.cursor = 0;
            self.attachments.clear();
            return;
        };
        self.restore_snapshot(snapshot);
    }

    fn restore_snapshot(&mut self, snapshot: ComposerSnapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.attachments = snapshot.attachments;
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

impl ComposerSnapshot {
    fn matches_entry(&self, other: &Self) -> bool {
        self.text == other.text && self.attachments == other.attachments
    }

    fn remove_attachments(&mut self) {
        self.text = self.attachments.history_text(&self.text);
        self.cursor = self.text.len();
        self.attachments.clear();
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
mod tests;
