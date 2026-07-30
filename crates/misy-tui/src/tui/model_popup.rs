//! Bounded centered-popup layout and rendering for provider and model pickers.

use super::{
    display_width::{text_width, truncate_to_width},
    popup,
    render::{list_line, modal_label_width, padded_line},
    state::{ModalPresentation, UiState, spinner_frame},
    style,
};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

const MAX_MODEL_ROWS: u16 = 20;
const MODEL_DETAIL_ROWS: u16 = 4;

pub(super) fn render(
    frame: &mut ratatui::Frame,
    screen: Rect,
    state: &UiState,
    preview: &ModalPresentation,
) {
    let Some((layout, tabs)) = popup::layout(screen, &preview.tabs, MAX_MODEL_ROWS) else {
        return;
    };
    let visible_rows = if preview.tabs.is_empty() {
        layout.content.height
    } else {
        layout
            .content
            .height
            .saturating_sub(MODEL_DETAIL_ROWS)
            .max(1)
    };
    let modal = state
        .modal_presentation(usize::from(visible_rows))
        .unwrap_or_else(|| preview.clone());
    let help = help_text(&modal, layout.inner.width);
    popup::render_shell(frame, &layout, &tabs, &modal.title, &help);
    if modal.loading {
        render_loading(frame, layout.content);
    } else {
        render_content(frame, layout.content, &modal);
    }
    render_auth_prompt_cursor(frame, layout.content, state, &modal);
}

fn render_auth_prompt_cursor(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &UiState,
    modal: &ModalPresentation,
) {
    let Some((row_index, value_column)) = state.auth_prompt_cursor_position() else {
        return;
    };
    let Some(row) = modal.rows.get(row_index) else {
        return;
    };
    let label_width = super::render::modal_label_width(&modal.rows, area.width);
    let prefix_width = super::display_width::text_width(&format!(
        "{} {}{}. ",
        if row.selected { "›" } else { " " },
        if row.current { "✓ " } else { "  " },
        row.number,
    ));
    let cursor_x = area
        .x
        .saturating_add(u16::try_from(prefix_width).unwrap_or(u16::MAX))
        .saturating_add(u16::try_from(label_width).unwrap_or(u16::MAX))
        .saturating_add(2)
        .saturating_add(u16::try_from(value_column).unwrap_or(u16::MAX))
        .min(area.right().saturating_sub(1));
    let cursor_y = area
        .y
        .saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX))
        .min(area.bottom().saturating_sub(1));
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_loading(frame: &mut ratatui::Frame, area: Rect) {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() / 100);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", spinner_frame(ticks)), style::accent()),
            Span::styled("Loading models…", style::muted()),
        ])),
        area,
    );
}

fn render_content(frame: &mut ratatui::Frame, area: Rect, modal: &ModalPresentation) {
    if let Some(operation) = &modal.operation {
        render_operation(frame, area, operation);
        return;
    }
    let label_width = modal_label_width(&modal.rows, area.width);
    let mut lines = Vec::new();
    for row in &modal.rows {
        if modal.tabs.is_empty() {
            lines.push(list_line(row, area.width, label_width));
        } else {
            lines.push(model_list_line(row, area.width, label_width));
            if row.selected {
                append_model_details(&mut lines, row);
            }
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_operation(frame: &mut ratatui::Frame, area: Rect, operation: &str) {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() / 100);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", spinner_frame(ticks)), style::accent()),
            Span::styled(operation.to_owned(), style::muted()),
        ])),
        area,
    );
}

fn model_list_line(
    row: &super::list::ListRowDisplay,
    width: u16,
    label_width: usize,
) -> Line<'static> {
    let marker_style = if row.selected {
        style::accent()
    } else {
        Style::default()
    };
    let current_style = if row.current {
        style::accent()
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::styled(if row.selected { "› " } else { "  " }, marker_style),
        Span::styled(if row.current { "✓ " } else { "  " }, current_style),
    ];
    let context_width = row
        .context
        .as_ref()
        .map_or(0, |context| text_width(context));
    let prefix_width = 4;
    let content_width = usize::from(width).saturating_sub(prefix_width);
    let label_budget = if row.context.is_some() {
        label_width.min(
            content_width
                .saturating_sub(context_width)
                .saturating_sub(2),
        )
    } else {
        content_width
    };
    spans.extend(styled_activity_label(&row.label, label_budget));
    if let Some(context) = &row.context {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(context.clone(), style::accent()));
    }
    padded_line(spans, width, Style::default())
}

fn append_model_details(lines: &mut Vec<Line<'static>>, row: &super::list::ListRowDisplay) {
    let Some(description) = &row.description else {
        return;
    };
    lines.extend(
        description
            .lines()
            .map(|detail| Line::styled(format!("      {detail}"), style::muted())),
    );
}

fn styled_activity_label(label: &str, width: usize) -> Vec<Span<'static>> {
    for (marker, marker_style) in [
        ("● ", style::success()),
        ("○ ", style::muted()),
        ("× ", style::error()),
        ("■ ", style::muted()),
    ] {
        if let Some(rest) = label.strip_prefix(marker) {
            let rest_width = width.saturating_sub(text_width(marker));
            let rest = padded_label(rest, rest_width);
            return vec![
                Span::styled(marker.to_owned(), marker_style),
                Span::raw(rest),
            ];
        }
    }
    vec![Span::raw(padded_label(label, width))]
}

fn padded_label(label: &str, width: usize) -> String {
    let label = truncate_to_width(label, width);
    let padding = width.saturating_sub(text_width(&label));
    format!("{label}{}", " ".repeat(padding))
}

fn help_text(modal: &ModalPresentation, width: u16) -> String {
    if let Some(hint) = &modal.help_hint {
        return if width < 42 { "esc" } else { hint.as_str() }.to_owned();
    }
    if !modal.tabs.is_empty() {
        return if width < 48 {
            "esc".to_owned()
        } else {
            "↑↓ select  ←→ section  enter apply  esc close".to_owned()
        };
    }
    let hint = if width < 42 {
        "esc"
    } else if modal.back_hint {
        "↑↓ select  enter apply  esc back"
    } else {
        "↑↓ select  enter open  esc close"
    };
    hint.to_owned()
}
