//! Ratatui widgets for the fullscreen transcript and interaction surfaces.

use super::{
    action::UiMode,
    composer::CommandPopupRow,
    list::ListRowDisplay,
    state::{ModalPresentation, TranscriptRow, UiState},
    style,
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

/// Renders the complete fullscreen client.
pub fn render(frame: &mut ratatui::Frame, state: &UiState) {
    render_with_composer_area(frame, state);
}

pub(super) fn render_with_composer_area(frame: &mut ratatui::Frame, state: &UiState) -> Rect {
    let area = frame.area();
    let popup_rows = state.command_popup_rows_for_render();
    let modal = state.modal_presentation(MAX_VIEW_ROWS);
    let composer_height = composer_height(state);
    let surface_height = surface_height(&popup_rows, modal.as_ref());
    let queued_prompts = state.queued_prompt_lines(MAX_QUEUED_PROMPT_ROWS);
    let busy = state.busy_label(Instant::now());
    let areas = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(queued_prompts.len()).unwrap_or(u16::MAX)),
        Constraint::Length(u16::from(busy.is_some())),
        Constraint::Length(composer_height),
        Constraint::Length(surface_height),
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
    render_composer(frame, areas[3], state);
    render_surface(frame, areas[4], &popup_rows, modal.as_ref());
    if let Some(modal) = modal.as_ref().filter(|modal| !modal.tabs.is_empty()) {
        super::model_popup::render(frame, area, state, modal);
    }
    render_footer(frame, areas[5], state);
    render_cursor(frame, areas[3], state);
    areas[3]
}

/// Converts transcript rows into styled terminal lines.
pub(super) fn transcript_lines(rows: &[TranscriptRow]) -> Vec<Line<'static>> {
    rows.iter().flat_map(row_lines).collect()
}

fn composer_height(state: &UiState) -> u16 {
    u16::try_from(state.composer_line_count())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
}

fn surface_height(popup_rows: &[CommandPopupRow], modal: Option<&ModalPresentation>) -> u16 {
    if !popup_rows.is_empty() {
        return POPUP_TOP_SPACE.saturating_add(u16::try_from(popup_rows.len()).unwrap_or(u16::MAX));
    }
    let Some(modal) = modal.filter(|modal| modal.tabs.is_empty()) else {
        return 0;
    };
    let row_count = if modal.operation.is_some() {
        1
    } else {
        MAX_VIEW_ROWS
    };
    let back_hint = if modal.back_hint { 2 } else { 0 };
    u16::try_from(2 + row_count + back_hint).unwrap_or(u16::MAX)
}

fn render_transcript(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    if area.is_empty() {
        return;
    }
    let mut lines = state.startup_header.lines(area.width);
    lines.extend(transcript_lines(state.transcript()));
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let total_height = u16::try_from(paragraph.line_count(area.width)).unwrap_or(u16::MAX);
    let scroll = total_height.saturating_sub(area.height);
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
}

fn row_lines(row: &TranscriptRow) -> Vec<Line<'static>> {
    match row {
        TranscriptRow::Provider { id, authenticated } => {
            vec![provider_status_line(id, *authenticated)]
        }
        TranscriptRow::Model {
            provider,
            id,
            selected,
        } => vec![model_status_line(provider, id, *selected)],
        TranscriptRow::UserPrompt(prompt) => user_prompt_lines(prompt),
        TranscriptRow::AssistantText(text) => text
            .split('\n')
            .map(|line| Line::raw(line.to_owned()))
            .collect(),
        TranscriptRow::ToolCall {
            name, arguments, ..
        } => vec![tool_call_line(name, arguments.as_deref())],
        TranscriptRow::ToolResult {
            is_error, content, ..
        } => tool_result_lines(*is_error, content.as_deref()),
        TranscriptRow::Info(message) => vec![Line::styled(format!("  {message}"), style::muted())],
        TranscriptRow::Error(message) => vec![Line::styled(format!("  {message}"), style::error())],
    }
}

fn provider_status_line(id: &str, authenticated: bool) -> Line<'static> {
    let status = if authenticated {
        "authenticated"
    } else {
        "offline"
    };
    Line::styled(format!("  provider {id}: {status}"), style::muted())
}

fn model_status_line(provider: &str, id: &str, selected: bool) -> Line<'static> {
    let marker = if selected { " ✓" } else { "" };
    Line::styled(format!("  model: {provider}/{id}{marker}"), style::muted())
}

fn user_prompt_lines(prompt: &str) -> Vec<Line<'static>> {
    prompt
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let marker = if index == 0 { "• " } else { "  " };
            Line::from(vec![
                Span::styled(marker, style::muted()),
                Span::styled(line.to_owned(), style::muted()),
            ])
        })
        .collect()
}

fn tool_call_line(name: &str, arguments: Option<&str>) -> Line<'static> {
    Line::from(vec![
        Span::styled("⏺ ", style::accent()),
        Span::raw(format!("{name}({})", arguments.unwrap_or_default())),
    ])
}

fn tool_result_lines(is_error: bool, content: Option<&str>) -> Vec<Line<'static>> {
    let row_style = if is_error {
        style::error()
    } else {
        style::muted()
    };
    let fallback = if is_error { "tool failed" } else { "completed" };
    content
        .unwrap_or(fallback)
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { "  ⎿ " } else { "    " };
            Line::styled(format!("{prefix}{line}"), row_style)
        })
        .collect()
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

fn render_surface(
    frame: &mut ratatui::Frame,
    area: Rect,
    popup_rows: &[CommandPopupRow],
    modal: Option<&ModalPresentation>,
) {
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
        return;
    }
    let Some(modal) = modal else {
        return;
    };
    if !modal.tabs.is_empty() {
        return;
    }
    let mut lines = vec![
        Line::styled(format!("  {}", modal.title), style::muted()),
        Line::raw(""),
    ];
    if let Some(operation) = &modal.operation {
        lines.push(Line::from(vec![
            Span::styled("  ⠋ ", style::accent()),
            Span::styled(operation.clone(), style::muted()),
        ]));
    } else {
        let label_width = modal_label_width(&modal.rows, area.width);
        lines.extend(
            modal
                .rows
                .iter()
                .map(|row| list_line(row, area.width, label_width)),
        );
    }
    if modal.back_hint {
        lines.push(Line::raw(""));
        lines.push(Line::styled("  Esc back", style::muted()));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
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

fn list_line(row: &ListRowDisplay, width: u16, label_width: usize) -> Line<'static> {
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
    let mut spans = vec![
        Span::styled(format!("{marker} {current}{}. ", row.number), selected),
        Span::styled(format!("{:<label_width$}", row.label), selected),
    ];
    if !description.is_empty() {
        spans.push(Span::styled(
            description.to_owned(),
            description_style(description, muted, row.selected),
        ));
    }
    padded_line(spans, width, selected)
}

pub(super) fn modal_label_width(rows: &[ListRowDisplay], width: u16) -> usize {
    let maximum = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(1);
    maximum.min(usize::from(width).saturating_sub(18).max(1))
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

fn render_cursor(frame: &mut ratatui::Frame, area: Rect, state: &UiState) {
    if state.mode() != UiMode::Input || area.width < 3 || area.height < 3 {
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
