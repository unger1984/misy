//! Ratatui widgets for the fullscreen transcript and interaction surfaces.

use super::{
    action::UiMode,
    bottom_surface::{
        minimum_question_height, question_height, question_rows, render_question_surface,
        render_todos, todo_height,
    },
    composer::CommandPopupRow,
    display_width::{text_width, truncate_to_width},
    list::ListRowDisplay,
    state::UiState,
    style,
    transcript_render::transcript_lines,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use std::time::Instant;

const MAX_VIEW_ROWS: usize = 8;
const MAX_QUEUED_PROMPT_ROWS: usize = 3;
const POPUP_TOP_SPACE: u16 = 1;
const TRANSCRIPT_INSET: u16 = 2;

/// Renders the complete fullscreen client.
pub fn render(frame: &mut ratatui::Frame, state: &UiState) {
    render_with_composer_area(frame, state);
}

pub(super) fn render_with_composer_area(frame: &mut ratatui::Frame, state: &UiState) -> Rect {
    let area = frame.area();
    if state.mode() == UiMode::ActivityDetail {
        render_activity_log(frame, area, state);
        return area;
    }
    let question_active = state.question_presentation(1).is_some();
    let popup_rows = if question_active {
        Vec::new()
    } else {
        state.command_popup_rows_for_render()
    };
    let modal = state.modal_presentation(MAX_VIEW_ROWS);
    let composer_height = if question_active {
        0
    } else {
        composer_height(state)
    };
    let surface_height = surface_height(&popup_rows);
    let queued_prompts = state.queued_prompt_lines(MAX_QUEUED_PROMPT_ROWS);
    let busy = state.busy_label(Instant::now());
    let reserved_height = u16::try_from(queued_prompts.len())
        .unwrap_or(u16::MAX)
        .saturating_add(u16::from(busy.is_some()))
        .saturating_add(surface_height)
        .saturating_add(u16::from(state.activity_bar_visible()))
        .saturating_add(1);
    let bottom_budget = area.height.saturating_sub(reserved_height);
    let (todo_height, question_height, composer_height) =
        bottom_heights(state, bottom_budget, composer_height, question_active);
    let areas = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(queued_prompts.len()).unwrap_or(u16::MAX)),
        Constraint::Length(u16::from(busy.is_some())),
        Constraint::Length(todo_height),
        Constraint::Length(question_height),
        Constraint::Length(composer_height),
        Constraint::Length(surface_height),
        Constraint::Length(u16::from(state.activity_bar_visible())),
        Constraint::Length(1),
    ])
    .split(area);

    render_transcript(frame, areas[0], state);
    if !queued_prompts.is_empty() {
        let lines = queued_prompts
            .into_iter()
            .map(|line| Line::styled(line, style::muted()))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(lines)), areas[1]);
    }
    if let Some(label) = busy {
        frame.render_widget(
            Paragraph::new(Line::styled(label, style::accent())),
            areas[2],
        );
    }
    render_todos(frame, areas[3], state);
    if let Some(probe) = state.question_presentation(1) {
        let visible_rows = question_rows(areas[4], probe.tabs.len() > 1);
        if let Some(question) = state.question_presentation(visible_rows) {
            render_question_surface(frame, areas[4], &question);
        }
    }
    render_composer(frame, areas[5], state);
    render_surface(frame, areas[6], &popup_rows);
    if state.activity_bar_visible() {
        render_activity_bar(frame, areas[7], state);
    }
    if let Some(modal) = modal.as_ref() {
        super::model_popup::render(frame, area, state, modal);
    }
    if let Some(view) = state.context_view() {
        super::context_popup::render(frame, area, view);
    }
    render_footer(frame, areas[8], state);
    render_cursor(frame, areas[5], state);
    areas[5]
}

fn bottom_heights(
    state: &UiState,
    budget: u16,
    composer_desired: u16,
    question_active: bool,
) -> (u16, u16, u16) {
    let todo_desired = todo_height(state, budget);
    if !question_active {
        let composer = composer_desired.min(budget);
        let todos = todo_desired.min(budget.saturating_sub(composer));
        return (todos, 0, composer);
    }

    let question_minimum = minimum_question_height(state).min(budget);
    let remaining_after_minimum = budget.saturating_sub(question_minimum);
    let todos = todo_desired.min(remaining_after_minimum);
    let remaining = remaining_after_minimum.saturating_sub(todos);
    let question_desired = question_height(state, budget);
    let question = question_minimum.saturating_add(
        question_desired
            .saturating_sub(question_minimum)
            .min(remaining),
    );
    (todos, question, 0)
}

fn render_activity_log(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(style::accent())
        .title(
            state
                .activity_log_title()
                .unwrap_or_else(|| "Task output".to_owned()),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let areas = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    let lines = state
        .activity_log_lines(usize::from(areas[0].height))
        .into_iter()
        .map(Line::raw)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            "↑↓ scroll  pgup/pgdn page  esc back",
            style::muted(),
        )),
        areas[1],
    );
}

fn composer_height(state: &UiState) -> u16 {
    u16::try_from(state.composer_line_count())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
}

fn surface_height(popup_rows: &[CommandPopupRow]) -> u16 {
    if !popup_rows.is_empty() {
        return POPUP_TOP_SPACE.saturating_add(u16::try_from(popup_rows.len()).unwrap_or(u16::MAX));
    }
    0
}

fn render_transcript(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    if area.is_empty() {
        return;
    }
    let mut lines = state.startup_header.lines(area.width);
    let transcript_width = area.width.saturating_sub(TRANSCRIPT_INSET);
    lines.extend(
        transcript_lines(
            state.transcript(),
            transcript_width,
            state.tool_output_expanded(),
            &state.transcript_expand_hint,
        )
        .into_iter()
        .map(inset_transcript_line),
    );
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let total_height = u16::try_from(paragraph.line_count(area.width)).unwrap_or(u16::MAX);
    let scroll = total_height.saturating_sub(area.height);
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
}

fn inset_transcript_line(mut line: Line<'static>) -> Line<'static> {
    line.spans
        .insert(0, Span::raw(" ".repeat(usize::from(TRANSCRIPT_INSET))));
    line
}

fn render_composer(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    let focused = state.mode() == UiMode::Input;
    let border_style = if focused {
        style::accent()
    } else {
        style::muted()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    let lines = composer_lines(state);
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn composer_lines(state: &UiState) -> Vec<Line<'static>> {
    let text = state.composer_input();
    if text.is_empty() {
        return vec![Line::from(vec![
            Span::styled("> ", style::accent()),
            Span::styled("Ask anything, / for commands", style::muted()),
        ])];
    }
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prompt = if index == 0 { "> " } else { "  " };
            Line::from(vec![
                Span::styled(prompt, style::accent()),
                Span::raw(line.to_owned()),
            ])
        })
        .collect()
}

fn render_surface(frame: &mut ratatui::Frame, area: Rect, popup_rows: &[CommandPopupRow]) {
    if !popup_rows.is_empty() {
        let popup_area = Rect::new(
            area.x,
            area.y.saturating_add(POPUP_TOP_SPACE),
            area.width,
            area.height.saturating_sub(POPUP_TOP_SPACE),
        );
        let lines = popup_rows
            .iter()
            .map(|row| popup_line(row, popup_area.width))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(lines)), popup_area);
    }
}

fn popup_line(row: &CommandPopupRow, width: u16) -> Line<'static> {
    let selected = if row.selected {
        style::selected()
    } else {
        Style::default()
    };
    let muted = if row.selected {
        style::selected_muted()
    } else {
        style::muted()
    };
    let marker = if row.selected { "›" } else { " " };
    padded_line(
        vec![
            Span::styled(format!("{marker} "), selected),
            Span::styled(format!("{:<10}", row.name), selected),
            Span::styled(row.description, muted),
        ],
        width,
        selected,
    )
}

pub(super) fn list_line(row: &ListRowDisplay, width: u16, label_width: usize) -> Line<'static> {
    let selected = if row.selected {
        style::selected()
    } else {
        Style::default()
    };
    let muted = if row.selected {
        style::selected_muted()
    } else {
        style::muted()
    };
    let marker = if row.selected { "›" } else { " " };
    let current = if row.current { "✓ " } else { "  " };
    let description = row.description.as_deref().unwrap_or_default();
    let prefix = format!("{marker} {current}{}. ", row.number);
    let available = usize::from(width).saturating_sub(text_width(&prefix));
    let (label_budget, description_budget) = column_widths(
        label_width,
        text_width(description),
        available,
        !description.is_empty(),
    );
    let label = truncate_to_width(&row.label, label_budget);
    let label_padding = label_budget.saturating_sub(text_width(&label));
    let mut spans = vec![
        Span::styled(prefix, selected),
        Span::styled(format!("{label}{}", " ".repeat(label_padding)), selected),
    ];
    if !description.is_empty() && description_budget != 0 {
        let description = truncate_to_width(description, description_budget);
        spans.push(Span::styled(
            format!("  {description}"),
            description_style(
                row.description.as_deref().unwrap_or_default(),
                muted,
                row.selected,
            ),
        ));
    }
    padded_line(spans, width, selected)
}

pub(super) fn modal_label_width(rows: &[ListRowDisplay], width: u16) -> usize {
    let maximum = rows
        .iter()
        .map(|row| text_width(&row.label))
        .max()
        .unwrap_or(1);
    maximum.min(usize::from(width).saturating_sub(18).max(1))
}

fn column_widths(
    label_width: usize,
    description_width: usize,
    available: usize,
    has_description: bool,
) -> (usize, usize) {
    if !has_description {
        return (label_width.min(available), 0);
    }
    let content = available.saturating_sub(2);
    let mut label = label_width.min(content.div_ceil(2));
    let mut description = description_width.min(content.saturating_sub(label));
    let spare = content.saturating_sub(label + description);
    let label_growth = label_width.saturating_sub(label).min(spare);
    label += label_growth;
    description += description_width
        .saturating_sub(description)
        .min(spare.saturating_sub(label_growth));
    (label, description)
}

fn description_style(description: &str, muted: Style, selected: bool) -> Style {
    if description.starts_with('✓') {
        if selected {
            style::selected_success()
        } else {
            style::success()
        }
    } else {
        muted
    }
}

pub(super) fn padded_line(
    mut spans: Vec<Span<'static>>,
    width: u16,
    style: Style,
) -> Line<'static> {
    let padding = usize::from(width).saturating_sub(Line::from(spans.clone()).width());
    if padding != 0 {
        spans.push(Span::styled(" ".repeat(padding), style));
    }
    Line::from(spans)
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    let quit_hint = state.quit_shortcut_active(Instant::now());
    let left = if quit_hint {
        "  press Ctrl+C again to exit"
    } else {
        "  ? for shortcuts"
    };
    let right = state.status_text();
    let gap = usize::from(area.width)
        .saturating_sub(Line::raw(left).width())
        .saturating_sub(Line::raw(&right).width());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                left,
                if quit_hint {
                    style::accent()
                } else {
                    style::muted()
                },
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, style::muted()),
        ])),
        area,
    );
}

fn render_activity_bar(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    frame.render_widget(
        Paragraph::new(Line::styled(
            state.activity_bar_label(),
            if state.activity_bar_focused {
                style::accent()
            } else {
                style::muted()
            },
        )),
        area,
    );
}

fn render_cursor(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    if state.mode() != UiMode::Input
        || state.activity_bar_focused
        || area.width < 3
        || area.height < 3
    {
        return;
    }
    let (column, row) = state.composer_cursor_position();
    let cursor_x = area
        .x
        .saturating_add(3)
        .saturating_add(column)
        .min(area.right().saturating_sub(2));
    let cursor_y = area
        .y
        .saturating_add(1)
        .saturating_add(row)
        .min(area.bottom().saturating_sub(2));
    frame.set_cursor_position((cursor_x, cursor_y));
}

#[cfg(test)]
mod tests;
