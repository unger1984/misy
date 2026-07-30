//! Saved-session rows and filtering for the resume picker.

use super::list::{ListRow, ListRowDisplay, ListView};
use misy_core::SessionSummary;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionPicker {
    list: ListView<String>,
}

impl SessionPicker {
    pub(super) fn new(sessions: Vec<SessionSummary>, current: Option<&str>) -> Self {
        let rows = if sessions.is_empty() {
            vec![ListRow::informational(
                "No saved sessions in this directory",
            )]
        } else {
            sessions
                .into_iter()
                .map(|session| row(session, current))
                .collect()
        };
        Self {
            list: ListView::new("Resume session", rows),
        }
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        self.list.insert_filter(text);
    }

    pub(super) fn backspace_filter(&mut self) {
        self.list.backspace_filter();
    }

    pub(super) fn move_up(&mut self) {
        self.list.move_up();
    }

    pub(super) fn move_down(&mut self) {
        self.list.move_down();
    }

    pub(super) fn selected_id(&self) -> Option<&String> {
        self.list.selected_value()
    }

    pub(super) fn select_number(&mut self, number: usize) -> bool {
        self.list.select_number(number)
    }

    pub(super) fn labels(&self) -> Vec<String> {
        self.list.labels()
    }

    pub(super) fn visible_rows(&self, visible_rows: usize) -> Vec<ListRowDisplay> {
        self.list.visible_rows(visible_rows)
    }
}

fn row(session: SessionSummary, current: Option<&str>) -> ListRow<String> {
    let short_id: String = session.id.chars().take(16).collect();
    let description = format!("{} · {short_id}", relative_time(session.modified_at));
    let search = format!("{} {}", session.preview, session.id);
    if current == Some(session.id.as_str()) {
        ListRow::current_with_search(session.id, session.preview, Some(description), search)
    } else {
        ListRow::selectable_with_search(session.id, session.preview, Some(description), search)
    }
}

fn relative_time(timestamp: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let elapsed = now.saturating_sub(timestamp);
    match elapsed {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{}m ago", elapsed / 60),
        3_600..=86_399 => format!("{}h ago", elapsed / 3_600),
        _ => format!("{}d ago", elapsed / 86_400),
    }
}
