//! Inline question and sticky todo rendering for the bottom interaction slot.

use super::{
    display_width::truncate_to_width,
    question_dialog::InlineQuestionPresentation,
    render::{list_line, modal_label_width, padded_line},
    state::UiState,
    style,
};
use misy_core::TodoStatus;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
};

const MAX_QUESTION_ROWS: usize = 8;

pub(super) fn todo_height(state: &UiState, available: u16) -> u16 {
    if state.snapshot.todos.is_empty() {
        return 0;
    }
    u16::try_from(state.snapshot.todos.len())
        .unwrap_or(u16::MAX)
        .min(available)
}

pub(super) fn question_height(state: &UiState, available: u16) -> u16 {
    let Some(question) = state.question_presentation(MAX_QUESTION_ROWS) else {
        return 0;
    };
    let fixed_rows = 4_u16.saturating_add(u16::from(question.tabs.len() > 1));
    let rows = u16::try_from(question.rows.len()).unwrap_or(u16::MAX);
    fixed_rows.saturating_add(rows).min(available)
}

pub(super) fn minimum_question_height(state: &UiState) -> u16 {
    let Some(question) = state.question_presentation(1) else {
        return 0;
    };
    5_u16.saturating_add(u16::from(question.tabs.len() > 1))
}

pub(super) fn question_rows(area: Rect, has_tabs: bool) -> usize {
    let fixed_rows = 4_u16.saturating_add(u16::from(has_tabs));
    usize::from(area.height.saturating_sub(fixed_rows)).max(1)
}

pub(super) fn render_todos(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    if area.is_empty() {
        return;
    }
    let lines = state
        .snapshot
        .todos
        .iter()
        .take(usize::from(area.height))
        .map(|todo| todo_line(todo, area.width))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

pub(super) fn render_question_surface(
    frame: &mut ratatui::Frame,
    area: Rect,
    question: &InlineQuestionPresentation,
) {
    if area.is_empty() {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style::accent());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let tab_height = u16::from(question.tabs.len() > 1 && inner.height > 1);
    let rows_height = inner.height.saturating_sub(2).saturating_sub(tab_height);
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(tab_height),
        Constraint::Min(0),
        Constraint::Length(u16::from(inner.height > 1)),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(Line::styled(
            truncate_to_width(&question.title, usize::from(inner.width)),
            style::accent(),
        )),
        layout[0],
    );
    if tab_height != 0 {
        frame.render_widget(
            Paragraph::new(question_tabs(&question.tabs, inner.width)),
            layout[1],
        );
    }
    let label_width = modal_label_width(&question.rows, inner.width);
    let lines = question
        .rows
        .iter()
        .take(usize::from(rows_height))
        .map(|row| list_line(row, inner.width, label_width))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), layout[2]);
    if inner.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                truncate_to_width(&question.help_hint, usize::from(inner.width)),
                style::muted(),
            )),
            layout[3],
        );
    }
}

fn todo_line(todo: &misy_core::TodoItem, width: u16) -> Line<'static> {
    let (marker, marker_style, text_style) = match &todo.status {
        TodoStatus::Pending => ("○ ", style::muted(), style::muted()),
        TodoStatus::InProgress => ("◉ ", style::accent(), style::accent()),
        TodoStatus::Done => (
            "✓ ",
            style::success(),
            style::muted().add_modifier(ratatui::style::Modifier::CROSSED_OUT),
        ),
    };
    let title = truncate_to_width(&todo.title, usize::from(width).saturating_sub(2));
    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(title, text_style),
    ])
}

fn question_tabs(tabs: &[(String, bool)], width: u16) -> Line<'static> {
    let available = usize::from(width);
    if tabs.is_empty() || available < 3 {
        return padded_line(Vec::new(), width, Style::default());
    }
    let visible = if available < tabs.len().saturating_mul(3) {
        vec![tabs.iter().find(|(_, active)| *active).unwrap_or(&tabs[0])]
    } else {
        tabs.iter().collect::<Vec<_>>()
    };
    let base_width = available / visible.len();
    let remainder = available % visible.len();
    let spans = visible
        .into_iter()
        .enumerate()
        .map(|(index, (label, active))| {
            let slot_width = base_width + usize::from(index < remainder);
            let label = truncate_to_width(label, slot_width.saturating_sub(2));
            Span::styled(
                format!(" {label} "),
                if *active {
                    style::model_tab_active()
                } else {
                    style::muted()
                },
            )
        })
        .collect();
    padded_line(spans, width, Style::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn todo(status: TodoStatus) -> misy_core::TodoItem {
        misy_core::TodoItem {
            title: "Task".to_owned(),
            status,
        }
    }

    #[test]
    fn todo_statuses_use_kimi_style_markers_and_emphasis() {
        let pending = todo_line(&todo(TodoStatus::Pending), 20);
        assert_eq!(pending.spans[0].content, "○ ");
        assert!(pending.spans[1].style.add_modifier.contains(Modifier::DIM));

        let active = todo_line(&todo(TodoStatus::InProgress), 20);
        assert_eq!(active.spans[0].content, "◉ ");
        assert_eq!(active.spans[1].style.fg, Some(Color::Cyan));
        assert!(active.spans[1].style.add_modifier.contains(Modifier::BOLD));

        let done = todo_line(&todo(TodoStatus::Done), 20);
        assert_eq!(done.spans[0].content, "✓ ");
        assert_eq!(done.spans[0].style.fg, Some(Color::Green));
        assert!(
            done.spans[1]
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn long_question_header_remains_visible_as_a_truncated_tab() {
        let line = question_tabs(&[("Подробный способ проверки".to_owned(), true)], 14);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(line.width() <= 14);
        assert!(rendered.contains('…'), "{rendered:?}");
        assert!(
            line.spans
                .iter()
                .any(|span| span.style == style::model_tab_active())
        );
    }
}
