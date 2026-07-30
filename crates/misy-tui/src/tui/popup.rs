//! Shared centered-popup geometry, frame, tabs, and help rendering.

use super::{render::padded_line, style};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

const MAX_POPUP_WIDTH: u16 = 100;
const POPUP_FIXED_ROWS: u16 = 15;

pub(super) struct PopupLayout {
    pub(super) area: Rect,
    pub(super) inner: Rect,
    pub(super) content: Rect,
}

pub(super) fn layout(
    screen: Rect,
    tabs: &[(String, bool)],
    maximum_content_rows: u16,
) -> Option<(PopupLayout, Vec<Line<'static>>)> {
    let compact = screen.width < 14 || screen.height < 9;
    let margin = u16::from(!compact);
    let available_width = screen.width.saturating_sub(margin.saturating_mul(2));
    let width = available_width.min(MAX_POPUP_WIDTH);
    if width < 6 || screen.height < 7 {
        return None;
    }
    let available_height = screen.height.saturating_sub(margin.saturating_mul(2));
    let max_tab_rows = available_height
        .saturating_sub(POPUP_FIXED_ROWS.saturating_add(1))
        .max(1);
    let tabs = visible_tab_lines(
        tabs_lines(tabs, width.saturating_sub(4)),
        usize::from(max_tab_rows),
    );
    let tab_height = u16::try_from(tabs.len()).unwrap_or(u16::MAX);
    let desired_height = POPUP_FIXED_ROWS
        .saturating_add(tab_height)
        .saturating_add(maximum_content_rows);
    let area = centered_rect(screen, width, available_height.min(desired_height));
    Some((layout_inside(area, tab_height), tabs))
}

pub(super) fn render_shell(
    frame: &mut ratatui::Frame,
    layout: &PopupLayout,
    tabs: &[Line<'static>],
    title: &str,
    help: &str,
) {
    frame.render_widget(Clear, layout.area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(style::accent()),
        layout.area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(title.to_owned(), style::accent())),
        Rect::new(layout.inner.x, layout.inner.y, layout.inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(Text::from(tabs.to_owned())),
        Rect::new(
            layout.inner.x,
            layout.inner.y.saturating_add(1),
            layout.inner.width,
            u16::try_from(tabs.len()).unwrap_or(u16::MAX),
        ),
    );
    let separator_y = layout.content.y.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "─".repeat(usize::from(layout.inner.width)),
            style::muted(),
        )),
        Rect::new(layout.inner.x, separator_y, layout.inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::styled(help.to_owned(), style::muted())),
        Rect::new(
            layout.inner.x,
            layout.inner.bottom().saturating_sub(1),
            layout.inner.width,
            1,
        ),
    );
}

fn layout_inside(area: Rect, tab_height: u16) -> PopupLayout {
    let inner = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let content_y = inner.y.saturating_add(2).saturating_add(tab_height);
    PopupLayout {
        area,
        inner,
        content: Rect::new(
            inner.x,
            content_y,
            inner.width,
            inner.bottom().saturating_sub(1).saturating_sub(content_y),
        ),
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
        let tab = Span::styled(
            format!(" {label} "),
            if *active {
                style::model_tab_active()
            } else {
                style::muted()
            },
        );
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

#[cfg(test)]
mod tests {
    use super::{Rect, layout_inside};

    #[test]
    fn popup_text_regions_keep_one_blank_column_inside_each_border() {
        let area = Rect::new(10, 4, 60, 24);
        let layout = layout_inside(area, 0);

        assert_eq!(layout.inner.x, area.x + 2);
        assert_eq!(layout.inner.right(), area.right() - 2);
        assert_eq!(layout.content.x, layout.inner.x);
    }
}
