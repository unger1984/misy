//! Semantic rendering for committed transcript rows.

use super::{state::TranscriptRow, style, tool_render};
use ratatui::text::{Line, Span};
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

/// Converts transcript rows into styled terminal lines.
pub(super) fn transcript_lines(
    rows: &[TranscriptRow],
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> Vec<Line<'static>> {
    let tools = tool_render::tool_blocks(rows, width, expanded, expand_hint);
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let block = tools
            .get(&index)
            .cloned()
            .or_else(|| semantic_block(row, width, expanded));
        let Some(block) = block else {
            continue;
        };
        if !lines.is_empty() && !matches!(row, TranscriptRow::WorkSeparator { .. }) {
            lines.push(Line::raw(String::new()));
        }
        lines.extend(block);
    }
    lines
}

fn semantic_block(row: &TranscriptRow, width: u16, expanded: bool) -> Option<Vec<Line<'static>>> {
    match row {
        TranscriptRow::Provider { id, authenticated } => {
            Some(vec![provider_status_line(id, *authenticated)])
        }
        TranscriptRow::Model {
            provider,
            id,
            selected,
        } => Some(vec![model_status_line(provider, id, *selected)]),
        TranscriptRow::UserPrompt(prompt) => Some(user_prompt_lines(prompt, width)),
        TranscriptRow::AssistantText(text) => Some(assistant_lines(text, width)),
        TranscriptRow::Info(message) => {
            Some(vec![Line::styled(format!("  {message}"), style::muted())])
        }
        TranscriptRow::Error(message) => Some(vec![Line::from(vec![
            Span::styled("× ", style::error()),
            Span::styled(message.clone(), style::error()),
        ])]),
        TranscriptRow::WorkSeparator { elapsed } => Some(vec![work_separator(*elapsed, width)]),
        TranscriptRow::Compaction(checkpoint) => {
            Some(compaction_block(checkpoint, width, expanded))
        }
        TranscriptRow::ToolCall { .. }
        | TranscriptRow::ToolResult { .. }
        | TranscriptRow::ActivityFinished(_) => None,
    }
}

fn compaction_block(
    checkpoint: &misy_core::CompactionCheckpoint,
    width: u16,
    expanded: bool,
) -> Vec<Line<'static>> {
    let label = format!(
        " Compacted context · {} → {} ",
        compact_tokens(checkpoint.tokens_before),
        compact_tokens(checkpoint.tokens_after)
    );
    let mut lines = vec![centered_divider(&label, width)];
    if expanded {
        let preview = bounded_summary_preview(&checkpoint.summary);
        let content_width = usize::from(width).saturating_sub(2).max(1);
        lines.extend(
            preview
                .lines()
                .take(12)
                .flat_map(|line| wrap_text(line, content_width))
                .map(|line| Line::styled(format!("  {line}"), style::muted())),
        );
    }
    lines
}

fn centered_divider(label: &str, width: u16) -> Line<'static> {
    let width = usize::from(width);
    if label.chars().count() >= width {
        return Line::styled(
            super::display_width::truncate_to_width(label, width),
            style::muted(),
        );
    }
    let left = width.saturating_sub(label.chars().count()) / 2;
    let right = width.saturating_sub(label.chars().count() + left);
    Line::styled(
        format!("{}{}{}", "─".repeat(left), label, "─".repeat(right)),
        style::muted(),
    )
}

fn compact_tokens(tokens: usize) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens.is_multiple_of(1_000) {
        format!("{}k", tokens / 1_000)
    } else {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    }
}

fn bounded_summary_preview(summary: &str) -> &str {
    const MAX_PREVIEW_BYTES: usize = 2 * 1024;
    if summary.len() <= MAX_PREVIEW_BYTES {
        return summary;
    }
    let mut end = MAX_PREVIEW_BYTES;
    while !summary.is_char_boundary(end) {
        end -= 1;
    }
    &summary[..end]
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

fn user_prompt_lines(prompt: &str, width: u16) -> Vec<Line<'static>> {
    let content_width = usize::from(width).saturating_sub(2).max(1);
    prompt
        .split('\n')
        .flat_map(|line| wrap_text(line, content_width))
        .enumerate()
        .map(|(index, line)| {
            let marker = if index == 0 { "› " } else { "  " };
            padded_line(
                vec![
                    Span::styled(marker, style::user_message()),
                    Span::styled(line, style::user_message()),
                ],
                width,
                style::user_message(),
            )
        })
        .collect()
}

fn assistant_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let content_width = usize::from(width).saturating_sub(2).max(1);
    text.split('\n')
        .flat_map(|line| wrap_text(line, content_width))
        .enumerate()
        .map(|(index, line)| {
            let marker = if index == 0 { "• " } else { "  " };
            Line::from(vec![Span::styled(marker, style::accent()), Span::raw(line)])
        })
        .collect()
}

fn work_separator(elapsed: Duration, width: u16) -> Line<'static> {
    let width = usize::from(width);
    if elapsed <= Duration::from_secs(60) {
        return Line::styled("─".repeat(width), style::muted());
    }
    let seconds = elapsed.as_secs();
    let label = format!(" Worked for {}m {}s ", seconds / 60, seconds % 60);
    if label.chars().count() >= width {
        return Line::styled(
            super::display_width::truncate_to_width(&label, width),
            style::muted(),
        );
    }
    let left = width.saturating_sub(label.chars().count()) / 2;
    let right = width.saturating_sub(label.chars().count() + left);
    Line::styled(
        format!("{}{}{}", "─".repeat(left), label, "─".repeat(right)),
        style::muted(),
    )
}

fn padded_line(
    mut spans: Vec<Span<'static>>,
    width: u16,
    row_style: ratatui::style::Style,
) -> Line<'static> {
    let used = Line::from(spans.clone()).width();
    let padding = usize::from(width).saturating_sub(used);
    if padding != 0 {
        spans.push(Span::styled(" ".repeat(padding), row_style));
    }
    Line::from(spans)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if character_width > width {
            if used != 0 {
                rows.push(String::new());
            }
            if let Some(row) = rows.last_mut() {
                row.push('…');
            }
            used = 1;
            continue;
        }
        if used != 0 && used + character_width > width {
            rows.push(String::new());
            used = 0;
        }
        if let Some(row) = rows.last_mut() {
            row.push(character);
        }
        used += character_width;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{transcript_lines, work_separator};
    use crate::tui::state::TranscriptRow;
    use std::time::Duration;

    fn compaction_row(summary: String) -> TranscriptRow {
        TranscriptRow::Compaction(misy_core::CompactionCheckpoint {
            summary,
            first_kept_index: 2,
            profile: misy_core::ModelProfile::new(
                misy_core::ModelRef::new(
                    misy_core::ProviderId::new("fixture"),
                    misy_core::ModelId::new("model"),
                ),
                None,
            ),
            tokens_before: 96_000,
            tokens_after: 18_000,
            trigger: "manual".to_owned(),
            timestamp_ms: 1,
            history_len: 4,
        })
    }

    fn text(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn semantic_blocks_have_one_separator_row() {
        let rows = vec![
            TranscriptRow::UserPrompt("hello".to_owned()),
            TranscriptRow::AssistantText("hi".to_owned()),
            TranscriptRow::Info("done".to_owned()),
        ];
        let lines = text(&transcript_lines(&rows, 20, false, "Ctrl+O"));
        assert_eq!(lines.iter().filter(|line| line.is_empty()).count(), 2);
    }

    #[test]
    fn work_duration_only_appears_after_one_minute() {
        assert!(!text(&[work_separator(Duration::from_secs(60), 40)])[0].contains("Worked"));
        assert!(
            text(&[work_separator(Duration::from_secs(61), 40)])[0].contains("Worked for 1m 1s")
        );
    }

    #[test]
    fn user_rows_fill_the_width_with_a_contrasting_background() {
        let rows = vec![TranscriptRow::UserPrompt(
            "wide prompt that wraps".to_owned(),
        )];
        let lines = transcript_lines(&rows, 12, false, "Ctrl+O");

        assert!(lines.len() > 1);
        for line in lines {
            assert_eq!(line.width(), 12);
            assert!(
                line.spans
                    .iter()
                    .all(|span| span.style.bg == Some(ratatui::style::Color::DarkGray))
            );
        }
    }

    #[test]
    fn compaction_divider_reveals_only_a_bounded_preview_when_expanded() {
        let rows = vec![compaction_row("summary line\n".repeat(300))];
        let compact = text(&transcript_lines(&rows, 80, false, "Ctrl+O"));
        let expanded = text(&transcript_lines(&rows, 80, true, "Ctrl+O"));

        assert_eq!(compact.len(), 1);
        assert!(compact[0].contains("Compacted context · 96k → 18k"));
        assert!(expanded.len() > compact.len());
        assert!(expanded.len() <= 13);
    }
}
