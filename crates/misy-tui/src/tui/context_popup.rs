//! Specialized `/context` report rendering inside the shared popup shell.

use super::{context_view::ContextView, popup, style};
use misy_core::{
    AgentRoleStatus, ContextCategory, ContextCategoryUsage, ContextReport, InstructionSourceStatus,
};
use ratatui::{
    layout::Rect,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

const MAX_CONTEXT_ROWS: u16 = 36;

pub(super) fn render(frame: &mut ratatui::Frame, screen: Rect, view: &ContextView) {
    let Some((layout, tabs)) = popup::layout(screen, &[], MAX_CONTEXT_ROWS) else {
        return;
    };
    popup::render_shell(
        frame,
        &layout,
        &tabs,
        "Context usage",
        "↑↓ scroll  pgup/pgdn page  home/end  esc close",
    );
    let paragraph = Paragraph::new(Text::from(report_lines(&view.report, layout.content.width)))
        .wrap(Wrap { trim: false });
    let total = paragraph.line_count(layout.content.width);
    let maximum = total.saturating_sub(usize::from(layout.content.height));
    let scroll = u16::try_from(view.effective_scroll(maximum)).unwrap_or(u16::MAX);
    frame.render_widget(paragraph.scroll((scroll, 0)), layout.content);
}

fn report_lines(report: &ContextReport, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let model = report
        .model_display_name
        .as_deref()
        .unwrap_or("not selected");
    let window = if report.context_window == 0 {
        "unknown".to_owned()
    } else {
        format!("{} tokens", report.context_window)
    };
    lines.push(pair("Model", model));
    lines.push(pair("Window", &window));
    lines.push(pair("State", &format!("{:?}", report.state)));
    let estimated = if report.context_window == 0 {
        format!("{} text tokens", report.estimated_tokens)
    } else {
        let window = report.context_window as usize;
        let percent = report
            .estimated_tokens
            .saturating_mul(100)
            .saturating_add(window.saturating_sub(1))
            / window;
        format!("{} text tokens ({percent}% used)", report.estimated_tokens)
    };
    lines.push(pair("Estimated", &estimated));
    if let Some(threshold) = report.auto_compaction_threshold {
        lines.push(pair(
            "Auto compact",
            &format!(
                "{threshold} tokens · reserve {}",
                report.compaction_reserve_tokens.unwrap_or_default()
            ),
        ));
    }
    if report.context_window > 0 && report.estimated_tokens >= report.context_window as usize {
        lines.push(Line::styled(
            "Warning: estimated usage is at or above the advertised context window",
            style::error(),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(usage_bar(report, width));
    for category in &report.categories {
        lines.push(category_line(category));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("AGENTS.md", style::accent()));
    if report.sources.is_empty() {
        lines.push(Line::styled(
            "  No active instruction files",
            style::muted(),
        ));
    }
    for source in &report.sources {
        let marker = match source.status {
            InstructionSourceStatus::Active => "●",
            InstructionSourceStatus::Truncated => "◐",
            InstructionSourceStatus::Blocked => "×",
        };
        let detail = if source.truncated {
            format!(
                "{} / {} bytes",
                source.retained_bytes, source.original_bytes
            )
        } else {
            format!("{} bytes", source.retained_bytes)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), source_style(source.status)),
            Span::raw(source.display_path.clone()),
            Span::styled(format!("  {detail}"), style::muted()),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(pair(
        "Images",
        &format!(
            "{} decoded images · {} bytes · token estimate unavailable",
            report.image_count, report.image_bytes
        ),
    ));
    if !report.warnings.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Warnings", style::error()));
        for warning in &report.warnings {
            lines.push(Line::styled(
                format!("  {}: {:?}", warning.source.display_path, warning.reason),
                style::error(),
            ));
        }
    }
    append_role_lines(&mut lines, report);
    lines
}

fn append_role_lines(lines: &mut Vec<Line<'static>>, report: &ContextReport) {
    lines.push(Line::raw(""));
    lines.push(Line::styled("Agent roles", style::accent()));
    for role in &report.roles {
        let (marker, role_style) = match role.status {
            AgentRoleStatus::Available => ("●", style::success()),
            AgentRoleStatus::Invalid => ("×", style::error()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), role_style),
            Span::raw(role.name.clone()),
            Span::styled(format!("  {:?}", role.source), style::muted()),
        ]));
        if let Some(warning) = &role.warning {
            lines.push(Line::styled(format!("    {warning}"), style::error()));
        }
    }
}

fn usage_bar(report: &ContextReport, width: u16) -> Line<'static> {
    let cells = usize::from(width.clamp(24, 48));
    let allocations = allocate_cells(report, cells);
    let mut spans = Vec::new();
    for (category, count) in allocations {
        spans.push(Span::styled("■".repeat(count), category_style(category)));
    }
    Line::from(spans)
}

fn allocate_cells(report: &ContextReport, cells: usize) -> Vec<(ContextCategory, usize)> {
    let values = report
        .categories
        .iter()
        .filter(|usage| usage.estimated_tokens > 0)
        .collect::<Vec<_>>();
    let total = values
        .iter()
        .map(|usage| usage.estimated_tokens)
        .sum::<usize>()
        .max(1);
    let mut allocations = values
        .iter()
        .map(|usage| {
            (
                usage.category,
                (usage.estimated_tokens.saturating_mul(cells) / total).max(1),
            )
        })
        .collect::<Vec<_>>();
    while allocations.iter().map(|(_, count)| *count).sum::<usize>() > cells {
        let Some((index, _)) = allocations
            .iter()
            .enumerate()
            .filter(|(_, (_, count))| *count > 1)
            .max_by_key(|(_, (_, count))| *count)
        else {
            break;
        };
        allocations[index].1 -= 1;
    }
    while allocations.iter().map(|(_, count)| *count).sum::<usize>() < cells {
        let index = values
            .iter()
            .enumerate()
            .max_by_key(|(_, usage)| usage.estimated_tokens)
            .map_or(0, |(index, _)| index);
        if allocations.is_empty() {
            break;
        }
        allocations[index].1 += 1;
    }
    allocations
}

fn pair(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), style::muted()),
        Span::raw(value.to_owned()),
    ])
}

fn category_line(usage: &ContextCategoryUsage) -> Line<'static> {
    let label = match usage.category {
        ContextCategory::MisyPrompt => "Misy prompt",
        ContextCategory::AgentsMd => "AGENTS.md",
        ContextCategory::CustomAgents => "Custom agents",
        ContextCategory::ToolDefinitions => "Tool schemas",
        ContextCategory::CompactionSummary => "Compaction",
        ContextCategory::Messages => "Messages",
        ContextCategory::FreeSpace => "Free space",
    };
    Line::from(vec![
        Span::styled("■ ", category_style(usage.category)),
        Span::raw(format!("{label:<14} {} tokens", usage.estimated_tokens)),
    ])
}

fn category_style(category: ContextCategory) -> ratatui::style::Style {
    match category {
        ContextCategory::MisyPrompt => style::accent(),
        ContextCategory::AgentsMd => style::success(),
        ContextCategory::CustomAgents => style::accent(),
        ContextCategory::ToolDefinitions => style::warning(),
        ContextCategory::CompactionSummary => style::warning(),
        ContextCategory::Messages => style::error(),
        ContextCategory::FreeSpace => style::muted(),
    }
}

fn source_style(status: InstructionSourceStatus) -> ratatui::style::Style {
    match status {
        InstructionSourceStatus::Active => style::success(),
        InstructionSourceStatus::Truncated => style::warning(),
        InstructionSourceStatus::Blocked => style::error(),
    }
}

#[cfg(test)]
mod tests {
    use super::allocate_cells;
    use misy_core::{ContextCategory, ContextCategoryUsage, ContextReport, ContextReportState};

    #[test]
    fn usage_grid_assigns_a_cell_to_every_nonzero_category() {
        let report = ContextReport {
            model: None,
            model_display_name: None,
            context_window: 100,
            estimated_tokens: 99,
            categories: vec![
                ContextCategoryUsage {
                    category: ContextCategory::MisyPrompt,
                    estimated_tokens: 98,
                },
                ContextCategoryUsage {
                    category: ContextCategory::AgentsMd,
                    estimated_tokens: 1,
                },
            ],
            state: ContextReportState::Base,
            sources: Vec::new(),
            image_count: 0,
            image_bytes: 0,
            warnings: Vec::new(),
            roles: Vec::new(),
            auto_compaction_threshold: None,
            compaction_reserve_tokens: None,
        };
        let cells = allocate_cells(&report, 24);
        assert_eq!(cells.iter().map(|(_, count)| *count).sum::<usize>(), 24);
        assert!(cells.iter().all(|(_, count)| *count > 0));
    }
}
