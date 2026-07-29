//! Bounded centered-popup layout and rendering for provider and model pickers.

use super::{
    render::{list_line, modal_label_width, padded_line},
    state::{ModalPresentation, UiState, spinner_frame},
    style,
};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

const MAX_POPUP_WIDTH: u16 = 100;
const MAX_MODEL_ROWS: u16 = 20;
const POPUP_FIXED_ROWS: u16 = 15;

struct PopupLayout {
    area: Rect,
    inner: Rect,
    tabs: Vec<Line<'static>>,
    separator: Rect,
    content: Rect,
    help: Rect,
}

pub(super) fn render(
    frame: &mut ratatui::Frame,
    screen: Rect,
    state: &UiState,
    preview: &ModalPresentation,
) {
    let Some(layout) = popup_layout(screen, &preview.tabs) else {
        return;
    };
    let modal = state
        .modal_presentation(usize::from(layout.content.height))
        .unwrap_or_else(|| preview.clone());
    frame.render_widget(Clear, layout.area);
    render_frame(frame, &layout, &modal);
    if modal.loading {
        render_loading(frame, layout.content);
    } else {
        render_content(frame, layout.content, &modal);
    }
}

fn render_frame(frame: &mut ratatui::Frame, layout: &PopupLayout, modal: &ModalPresentation) {
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(style::accent()),
        layout.area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(format!(" {}", modal.title), style::accent())),
        Rect::new(layout.inner.x, layout.inner.y, layout.inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(Text::from(layout.tabs.clone())),
        Rect::new(
            layout.inner.x,
            layout.inner.y.saturating_add(1),
            layout.inner.width,
            u16::try_from(layout.tabs.len()).unwrap_or(u16::MAX),
        ),
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            "─".repeat(usize::from(layout.inner.width)),
            style::muted(),
        )),
        layout.separator,
    );
    frame.render_widget(
        Paragraph::new(help_line(modal, layout.inner.width)),
        layout.help,
    );
}

fn popup_layout(screen: Rect, tabs: &[(String, bool)]) -> Option<PopupLayout> {
    let compact = screen.width < 14 || screen.height < 9;
    let margin = u16::from(!compact);
    let available_width = screen.width.saturating_sub(margin.saturating_mul(2));
    let width = available_width.min(MAX_POPUP_WIDTH);
    if width < 4 || screen.height < 7 {
        return None;
    }
    let available_height = screen.height.saturating_sub(margin.saturating_mul(2));
    let max_tab_rows = available_height
        .saturating_sub(POPUP_FIXED_ROWS.saturating_add(1))
        .max(1);
    let tab_lines = visible_tab_lines(
        tabs_lines(tabs, width.saturating_sub(2)),
        usize::from(max_tab_rows),
    );
    let tab_height = u16::try_from(tab_lines.len()).unwrap_or(u16::MAX);
    let desired_height = POPUP_FIXED_ROWS
        .saturating_add(tab_height)
        .saturating_add(MAX_MODEL_ROWS);
    let area = centered_rect(screen, width, available_height.min(desired_height));
    Some(layout_inside(area, tab_lines))
}

fn layout_inside(area: Rect, tabs: Vec<Line<'static>>) -> PopupLayout {
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let tab_height = u16::try_from(tabs.len()).unwrap_or(u16::MAX);
    let separator_y = inner.y.saturating_add(1).saturating_add(tab_height);
    let help_y = inner.bottom().saturating_sub(1);
    let content_y = separator_y.saturating_add(1);
    PopupLayout {
        area,
        inner,
        tabs,
        separator: Rect::new(inner.x, separator_y, inner.width, 1),
        content: Rect::new(
            inner.x,
            content_y,
            inner.width,
            help_y.saturating_sub(content_y),
        ),
        help: Rect::new(inner.x, help_y, inner.width, 1),
    }
}

fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        screen
            .x
            .saturating_add(screen.width.saturating_sub(width) / 2),
        screen
            .y
            .saturating_add(screen.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

fn tabs_lines(tabs: &[(String, bool)], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut line_width = 0;
    for (label, active) in tabs {
        let tab = tab_span(label, *active);
        let separator_width = usize::from(!spans.is_empty());
        if !spans.is_empty() && line_width + separator_width + tab.width() > usize::from(width) {
            lines.push(padded_line(spans, width, Style::default()));
            spans = Vec::new();
            line_width = 0;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
            line_width += 1;
        }
        line_width += tab.width();
        spans.push(tab);
    }
    if !spans.is_empty() {
        lines.push(padded_line(spans, width, Style::default()));
    }
    lines
}

fn visible_tab_lines(lines: Vec<Line<'static>>, maximum: usize) -> Vec<Line<'static>> {
    if lines.len() <= maximum {
        return lines;
    }
    let active = lines
        .iter()
        .position(|line| {
            line.spans
                .iter()
                .any(|span| span.style == style::model_tab_active())
        })
        .unwrap_or(0);
    let start = active
        .saturating_sub(maximum.saturating_sub(1))
        .min(lines.len().saturating_sub(maximum));
    lines.into_iter().skip(start).take(maximum).collect()
}

fn tab_span(label: &str, active: bool) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        if active {
            style::model_tab_active()
        } else {
            style::muted()
        },
    )
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
        Span::raw(format!("{:<label_width$}", row.label)),
    ];
    if let Some(description) = &row.description {
        spans.push(Span::styled(format!("  {description}"), style::muted()));
    }
    padded_line(spans, width, Style::default())
}

fn model_help_line(width: u16) -> Line<'static> {
    let hint = if width < 48 {
        "esc"
    } else {
        " ↑↓ select  ←→ section  enter apply  esc close"
    };
    Line::styled(hint, style::muted())
}

fn help_line(modal: &ModalPresentation, width: u16) -> Line<'static> {
    if !modal.tabs.is_empty() {
        return model_help_line(width);
    }
    let hint = if width < 42 {
        "esc"
    } else if modal.back_hint {
        " ↑↓ select  enter apply  esc back"
    } else {
        " ↑↓ select  enter open  esc close"
    };
    Line::styled(hint, style::muted())
}
