//! Bounded centered-popup layout and rendering for provider and model pickers.

use super::{
    popup,
    render::{list_line, modal_label_width, padded_line},
    state::{ModalPresentation, UiState, spinner_frame},
    style,
};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::Paragraph,
};

const MAX_MODEL_ROWS: u16 = 20;

pub(super) fn render(
    frame: &mut ratatui::Frame,
    screen: Rect,
    state: &UiState,
    preview: &ModalPresentation,
) {
    let Some((layout, tabs)) = popup::layout(screen, &preview.tabs, MAX_MODEL_ROWS) else {
        return;
    };
    let modal = state
        .modal_presentation(usize::from(layout.content.height))
        .unwrap_or_else(|| preview.clone());
    let help = help_text(&modal, layout.inner.width);
    popup::render_shell(frame, &layout, &tabs, &modal.title, &help);
    if modal.loading {
        render_loading(frame, layout.content);
    } else {
        render_content(frame, layout.content, &modal);
    }
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
    let lines = modal
        .rows
        .iter()
        .map(|row| {
            if modal.tabs.is_empty() {
                list_line(row, area.width, label_width)
            } else {
                model_list_line(row, area.width, label_width)
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
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
    spans.extend(styled_activity_label(&row.label, label_width));
    if let Some(description) = &row.description {
        spans.push(Span::styled(format!("  {description}"), style::muted()));
    }
    padded_line(spans, width, Style::default())
}

fn styled_activity_label(label: &str, width: usize) -> Vec<Span<'static>> {
    let padded = format!("{label:<width$}");
    for (marker, marker_style) in [
        ("● ", style::success()),
        ("○ ", style::muted()),
        ("× ", style::error()),
        ("■ ", style::muted()),
    ] {
        if let Some(rest) = padded.strip_prefix(marker) {
            return vec![
                Span::styled(marker.to_owned(), marker_style),
                Span::raw(rest.to_owned()),
            ];
        }
    }
    vec![Span::raw(padded)]
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
