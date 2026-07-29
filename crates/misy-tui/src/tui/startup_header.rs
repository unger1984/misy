//! Immutable startup card rendered as the first item in the transcript flow.

use super::{display_width::truncate_to_width, style};
use misy_core::ModelRef;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

const MAX_BOX_WIDTH: usize = 88;
const DUAL_COLUMN_MIN_WIDTH: usize = 64;

#[derive(Clone, Debug)]
pub(super) struct StartupHeader {
    model: String,
    directory: String,
}

impl StartupHeader {
    pub(super) fn new(model: Option<&ModelRef>, directory: Option<&Path>) -> Self {
        let model = model
            .map(|model| format!("{}/{}", model.provider.as_str(), model.model.as_str()))
            .unwrap_or_else(|| "not selected".to_owned());
        let directory = directory
            .map(display_directory)
            .unwrap_or_else(|| "unavailable".to_owned());
        Self { model, directory }
    }

    pub(super) fn lines(&self, terminal_width: u16) -> Vec<Line<'static>> {
        let box_width = usize::from(terminal_width)
            .saturating_sub(2)
            .min(MAX_BOX_WIDTH);
        if box_width < 4 {
            return Vec::new();
        }
        let inner_width = box_width - 2;
        let mut lines = vec![title_line(box_width)];
        if inner_width >= DUAL_COLUMN_MIN_WIDTH {
            lines.extend(self.dual_column_lines(inner_width));
        } else {
            lines.extend(self.single_column_lines(inner_width));
        }
        lines.push(Line::styled(
            format!(" ╰{}╯", "─".repeat(inner_width)),
            style::muted(),
        ));
        lines.push(Line::raw(""));
        lines
    }

    fn dual_column_lines(&self, inner_width: usize) -> Vec<Line<'static>> {
        let left_width = 28.min((inner_width - 1) / 2);
        let right_width = inner_width - left_width - 1;
        let model = format!("model: {}", self.model);
        let directory = format!("directory: {}", self.directory);
        [
            ("Welcome back!", "Tips for getting started"),
            ("", "Type a task to begin"),
            ("   ╭─────╮", "/provider  Connect an account"),
            ("   │ Misy│", "/model     Choose a model"),
            ("   ╰─────╯", "Shift+Enter  Insert a new line"),
            (model.as_str(), "?          Show shortcuts"),
            (directory.as_str(), ""),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (left, right))| {
            let left_alignment = if index <= 4 {
                Alignment::Center
            } else {
                Alignment::Left
            };
            let left_style = if index == 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else if index >= 5 {
                style::muted()
            } else {
                Style::default()
            };
            let right_style = if index == 0 {
                style::accent()
            } else {
                style::muted()
            };
            bordered_dual_line(
                left,
                left_width,
                left_alignment,
                left_style,
                right,
                right_width,
                right_style,
            )
        })
        .collect()
    }

    fn single_column_lines(&self, inner_width: usize) -> Vec<Line<'static>> {
        let model = format!(" model: {}", self.model);
        let directory = format!(" directory: {}", self.directory);
        [
            (
                "Welcome back!",
                Alignment::Center,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            ("Misy", Alignment::Center, style::accent()),
            (model.as_str(), Alignment::Left, style::muted()),
            (directory.as_str(), Alignment::Left, style::muted()),
            ("", Alignment::Left, Style::default()),
            (
                " / for commands · Shift+Enter for a new line",
                Alignment::Left,
                style::muted(),
            ),
        ]
        .into_iter()
        .map(|(text, alignment, row_style)| bordered_line(text, inner_width, alignment, row_style))
        .collect()
    }
}

fn display_directory(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return path.display().to_string();
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return path.display().to_string();
    };
    if relative.as_os_str().is_empty() {
        "~".to_owned()
    } else {
        format!("~{}{}", std::path::MAIN_SEPARATOR, relative.display())
    }
}

#[derive(Clone, Copy)]
enum Alignment {
    Left,
    Center,
}

fn title_line(box_width: usize) -> Line<'static> {
    let title = format!(" Misy v{} ", env!("CARGO_PKG_VERSION"));
    let available = box_width - 2;
    let title = truncate_to_width(&title, available.saturating_sub(1));
    let remaining = available.saturating_sub(1 + UnicodeWidthStr::width(title.as_str()));
    Line::from(vec![
        Span::styled(" ╭─", style::muted()),
        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}╮", "─".repeat(remaining)), style::muted()),
    ])
}

fn bordered_dual_line(
    left: &str,
    left_width: usize,
    left_alignment: Alignment,
    left_style: Style,
    right: &str,
    right_width: usize,
    right_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled(" │", style::muted())];
    spans.extend(cell_spans(left, left_width, left_alignment, left_style));
    spans.push(Span::styled("│", style::muted()));
    spans.extend(cell_spans(right, right_width, Alignment::Left, right_style));
    spans.push(Span::styled("│", style::muted()));
    Line::from(spans)
}

fn bordered_line(
    text: &str,
    width: usize,
    alignment: Alignment,
    row_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled(" │", style::muted())];
    spans.extend(cell_spans(text, width, alignment, row_style));
    spans.push(Span::styled("│", style::muted()));
    Line::from(spans)
}

fn cell_spans(
    text: &str,
    width: usize,
    alignment: Alignment,
    cell_style: Style,
) -> Vec<Span<'static>> {
    let text = truncate_to_width(text, width);
    let text_width = UnicodeWidthStr::width(text.as_str());
    let padding = width.saturating_sub(text_width);
    let left_padding = match alignment {
        Alignment::Left => 0,
        Alignment::Center => padding / 2,
    };
    let right_padding = padding - left_padding;
    vec![
        Span::raw(" ".repeat(left_padding)),
        Span::styled(text, cell_style),
        Span::raw(" ".repeat(right_padding)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_responsive_and_never_exceeds_terminal_width() {
        let path = std::path::PathBuf::from("/a/very/long/project/path");
        let header = StartupHeader::new(None, Some(&path));
        for width in [20, 48, 96] {
            let lines = header.lines(width);
            assert!(!lines.is_empty());
            assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
        }
    }

    #[test]
    fn directory_below_home_uses_a_compact_tilde_prefix() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let rendered = display_directory(&home.join("dev").join("misy"));
        assert_eq!(
            rendered,
            format!(
                "~{}dev{}misy",
                std::path::MAIN_SEPARATOR,
                std::path::MAIN_SEPARATOR
            )
        );
    }
}
