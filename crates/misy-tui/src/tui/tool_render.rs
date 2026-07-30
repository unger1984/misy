//! Width-aware presentation for tool calls and background activity output.

use super::{state::TranscriptRow, style};
use ratatui::{style::Style, text::Line};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use unicode_width::UnicodeWidthChar;

const COMPACT_ROWS: usize = 4;
const EXPANDED_ROWS: usize = 12;
const OUTPUT_PREFIX_WIDTH: usize = 4;

#[derive(Clone)]
struct OutputLine {
    text: String,
    style: Style,
}

pub(super) fn tool_blocks(
    rows: &[TranscriptRow],
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> BTreeMap<usize, Vec<Line<'static>>> {
    let (results_by_call, paired_results) = pair_results(rows);
    let mut blocks = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let lines = match row {
            TranscriptRow::ToolCall {
                name, arguments, ..
            } => Some(call_block(
                name,
                arguments.as_deref(),
                results_by_call.get(&index).copied(),
                width,
                expanded,
                expand_hint,
            )),
            TranscriptRow::ToolResult {
                is_error, content, ..
            } if !paired_results.contains(&index) => Some(orphan_result_block(
                *is_error,
                content.as_deref(),
                width,
                expanded,
                expand_hint,
            )),
            TranscriptRow::ActivityFinished(output) => {
                Some(activity_block(output, width, expanded, expand_hint))
            }
            _ => None,
        };
        if let Some(lines) = lines {
            blocks.insert(index, lines);
        }
    }
    blocks
}

fn pair_results(rows: &[TranscriptRow]) -> (HashMap<usize, &TranscriptRow>, HashSet<usize>) {
    let mut pending: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut results_by_call = HashMap::new();
    let mut paired_results = HashSet::new();
    for (index, row) in rows.iter().enumerate() {
        match row {
            TranscriptRow::ToolCall { id, .. } => pending.entry(id).or_default().push(index),
            TranscriptRow::ToolResult { id, .. } => {
                let Some(call_index) = pending.get_mut(id.as_str()).and_then(Vec::pop) else {
                    continue;
                };
                results_by_call.insert(call_index, row);
                paired_results.insert(index);
            }
            _ => {}
        }
    }
    (results_by_call, paired_results)
}

fn call_block(
    name: &str,
    arguments: Option<&str>,
    result: Option<&TranscriptRow>,
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> Vec<Line<'static>> {
    let (marker, marker_style) = match result {
        Some(TranscriptRow::ToolResult { is_error: true, .. }) => ("×", style::error()),
        Some(_) => ("●", style::success()),
        None => ("○", style::accent()),
    };
    let mut lines = vec![Line::from(vec![
        ratatui::text::Span::styled(format!("{marker} "), marker_style),
        ratatui::text::Span::raw(tool_header(name, arguments, result, width)),
    ])];
    let Some(TranscriptRow::ToolResult {
        is_error, content, ..
    }) = result
    else {
        return lines;
    };
    let output = tool_output(name, arguments, *is_error, content.as_deref());
    lines.extend(bounded_output(&output, width, expanded, expand_hint));
    lines
}

fn tool_header(
    name: &str,
    arguments: Option<&str>,
    result: Option<&TranscriptRow>,
    width: u16,
) -> String {
    if matches!(
        result,
        Some(TranscriptRow::ToolResult {
            is_error: false,
            ..
        })
    ) {
        match name {
            "SetTodoList" => return "Used TodoList".to_owned(),
            "AskUserQuestion" => return "Used AskUserQuestion".to_owned(),
            _ => {}
        }
    }
    friendly_header(name, arguments, width)
}

fn orphan_result_block(
    is_error: bool,
    content: Option<&str>,
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> Vec<Line<'static>> {
    let (marker, label, header_style) = if is_error {
        ("×", "Tool failed", style::error())
    } else {
        ("●", "Tool completed", style::success())
    };
    let mut lines = vec![Line::styled(format!("{marker} {label}"), header_style)];
    let output = content_output(content, is_error);
    lines.extend(bounded_output(&output, width, expanded, expand_hint));
    lines
}

fn activity_block(
    output: &misy_core::ActivityOutput,
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> Vec<Line<'static>> {
    let (marker, state, header_style) = match output.activity.status {
        misy_core::ActivityStatus::Completed => ("●", "completed", style::success()),
        misy_core::ActivityStatus::Failed => ("×", "failed", style::error()),
        misy_core::ActivityStatus::Stopped => ("■", "stopped", style::muted()),
        _ => ("○", "finished", style::muted()),
    };
    let mut lines = vec![Line::styled(
        format!(
            "{marker} Background task {state} · {} · {}",
            output.activity.id, output.activity.title
        ),
        header_style,
    )];
    let mut content = super::state::output_content_labels(output)
        .into_iter()
        .map(|text| OutputLine {
            text,
            style: style::muted(),
        })
        .collect::<Vec<_>>();
    if let Some(code) = output.activity.exit_code {
        content.push(OutputLine {
            text: format!("Process exited with code {code}"),
            style: if code == 0 {
                style::muted()
            } else {
                style::error()
            },
        });
    }
    if content.is_empty() {
        content.push(empty_output());
    }
    lines.extend(bounded_output(&content, width, expanded, expand_hint));
    lines
}

fn friendly_header(name: &str, arguments: Option<&str>, width: u16) -> String {
    let parsed = arguments.and_then(|arguments| serde_json::from_str::<Value>(arguments).ok());
    let value = |key: &str| {
        parsed
            .as_ref()
            .and_then(|arguments| arguments.get(key))
            .and_then(Value::as_str)
    };
    match name {
        "list_directory" => format!("List {}", value("path").unwrap_or("?")),
        "read_file" => format!("Read {}", value("path").unwrap_or("?")),
        "view_image" => format!("View image {}", value("path").unwrap_or("?")),
        "exec_command" => format!(
            "Run {}",
            value("cmd")
                .or_else(|| value("description").filter(|text| !text.trim().is_empty()))
                .unwrap_or("?")
        ),
        "write_file" => format!("Write {}", value("path").unwrap_or("?")),
        "task_list" => "List tasks".to_owned(),
        "task_stop" => format!("Stop task {}", value("task_id").unwrap_or("?")),
        "write_stdin" if value("chars").is_none_or(str::is_empty) => {
            format!("Poll task {}", value("task_id").unwrap_or("?"))
        }
        "write_stdin" => format!("Send input {}", value("task_id").unwrap_or("?")),
        "SetTodoList"
            if parsed
                .as_ref()
                .and_then(|value| value.get("todos"))
                .is_none() =>
        {
            "Read todo list".to_owned()
        }
        "SetTodoList" => "Update todo list".to_owned(),
        "AskUserQuestion" => "Ask user".to_owned(),
        _ => {
            let arguments = arguments.unwrap_or_default();
            let budget = usize::from(width).saturating_sub(name.len() + 10);
            format!(
                "Called {name}({})",
                super::display_width::truncate_to_width(arguments, budget)
            )
        }
    }
}

fn tool_output(
    name: &str,
    arguments: Option<&str>,
    is_error: bool,
    content: Option<&str>,
) -> Vec<OutputLine> {
    if !is_error {
        if name == "SetTodoList"
            && let Some(output) = todo_output(arguments, content)
        {
            return output;
        }
        if name == "AskUserQuestion"
            && let Some(output) = question_output(arguments, content)
        {
            return output;
        }
    }
    if name == "exec_command"
        && let Some(message) = background_start(content)
    {
        return vec![OutputLine {
            text: message,
            style: style::muted(),
        }];
    }
    content_output(content, is_error)
}

fn todo_output(arguments: Option<&str>, content: Option<&str>) -> Option<Vec<OutputLine>> {
    let arguments: Value = serde_json::from_str(arguments?).ok()?;
    let Some(todos) = arguments.get("todos") else {
        return Some(content_output(content, false));
    };
    if todos.is_null() {
        return Some(content_output(content, false));
    }
    let todos = todos.as_array()?;
    if todos.is_empty() {
        return Some(vec![muted_output("Todo list cleared")]);
    }
    todos
        .iter()
        .map(|todo| {
            Some(muted_output(format!(
                "- [{}] {}",
                todo.get("status")?.as_str()?,
                todo.get("title")?.as_str()?
            )))
        })
        .collect()
}

fn question_output(arguments: Option<&str>, content: Option<&str>) -> Option<Vec<OutputLine>> {
    let arguments: Value = serde_json::from_str(arguments?).ok()?;
    let questions = arguments.get("questions")?.as_array()?;
    let result: Value = serde_json::from_str(content?).ok()?;
    let answers = result.get("answers")?.as_object()?;
    if answers.is_empty() {
        return Some(vec![muted_output("Question dismissed")]);
    }
    questions
        .iter()
        .map(|question| {
            let text = question.get("question")?.as_str()?;
            let answer = answers.get(text)?.as_str()?;
            Some(muted_output(format!("{text}: {answer}")))
        })
        .collect()
}

fn muted_output(text: impl Into<String>) -> OutputLine {
    OutputLine {
        text: text.into(),
        style: style::muted(),
    }
}

fn background_start(content: Option<&str>) -> Option<String> {
    let value: Value = serde_json::from_str(content?).ok()?;
    if value.get("kind")?.as_str()? != "background" {
        return None;
    }
    Some(format!(
        "Running in background · {}",
        value.get("task_id")?.as_str()?
    ))
}

fn content_output(content: Option<&str>, is_error: bool) -> Vec<OutputLine> {
    let fallback = if is_error {
        "tool failed"
    } else {
        "(no output)"
    };
    let content = content.filter(|text| !text.is_empty()).unwrap_or(fallback);
    content
        .split('\n')
        .map(|text| OutputLine {
            text: text.to_owned(),
            style: if is_error {
                style::error()
            } else {
                style::muted()
            },
        })
        .collect()
}

fn empty_output() -> OutputLine {
    OutputLine {
        text: "(no output)".to_owned(),
        style: style::muted(),
    }
}

fn bounded_output(
    logical: &[OutputLine],
    width: u16,
    expanded: bool,
    expand_hint: &str,
) -> Vec<Line<'static>> {
    let budget = if expanded {
        EXPANDED_ROWS
    } else {
        COMPACT_ROWS
    };
    let content_width = usize::from(width)
        .saturating_sub(OUTPUT_PREFIX_WIDTH)
        .max(1);
    let wrapped = logical
        .iter()
        .map(|line| wrap_output_line(line, content_width))
        .collect::<Vec<_>>();
    if wrapped.iter().map(Vec::len).sum::<usize>() <= budget {
        return render_groups(&wrapped);
    }
    let Some((head, tail, marker)) =
        retained_groups(&wrapped, budget, content_width, expanded, expand_hint)
    else {
        return render_marker(
            &marker_text(logical.len(), expanded, expand_hint),
            content_width,
        );
    };
    let mut rows = render_groups(&wrapped[..head]);
    append_marker(&mut rows, &marker, content_width);
    append_groups(&mut rows, &wrapped[wrapped.len().saturating_sub(tail)..]);
    rows
}

fn retained_groups(
    wrapped: &[Vec<OutputLine>],
    budget: usize,
    content_width: usize,
    expanded: bool,
    expand_hint: &str,
) -> Option<(usize, usize, String)> {
    let all_marker = marker_text(wrapped.len(), expanded, expand_hint);
    if wrap_text(&all_marker, content_width).len() >= budget {
        return None;
    }
    for selected in (1..wrapped.len()).rev() {
        let hidden = wrapped.len() - selected;
        let marker = marker_text(hidden, expanded, expand_hint);
        let marker_rows = wrap_text(&marker, content_width).len();
        for head in balanced_heads(selected) {
            let tail = selected - head;
            let content_rows = wrapped[..head].iter().map(Vec::len).sum::<usize>()
                + wrapped[wrapped.len() - tail..]
                    .iter()
                    .map(Vec::len)
                    .sum::<usize>();
            if content_rows + marker_rows <= budget {
                return Some((head, tail, marker));
            }
        }
    }
    None
}

fn balanced_heads(selected: usize) -> Vec<usize> {
    if selected < 2 {
        return Vec::new();
    }
    let mut heads = (1..selected).collect::<Vec<_>>();
    heads.sort_by_key(|head| {
        (
            head.abs_diff(selected - head),
            usize::from(*head < selected - head),
        )
    });
    heads
}

fn marker_text(hidden: usize, expanded: bool, expand_hint: &str) -> String {
    if expanded {
        format!("… {hidden} more lines")
    } else {
        format!("… {hidden} more lines ({expand_hint} to expand)")
    }
}

fn wrap_output_line(line: &OutputLine, width: usize) -> Vec<OutputLine> {
    wrap_text(&line.text.replace('\t', "    "), width)
        .into_iter()
        .map(|text| OutputLine {
            text,
            style: line.style,
        })
        .collect()
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

fn render_groups(groups: &[Vec<OutputLine>]) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    append_groups(&mut rows, groups);
    rows
}

fn render_marker(marker: &str, width: usize) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    append_marker(&mut rows, marker, width);
    rows
}

fn append_marker(rows: &mut Vec<Line<'static>>, marker: &str, width: usize) {
    let group = wrap_text(marker, width)
        .into_iter()
        .map(|text| OutputLine {
            text,
            style: style::muted(),
        })
        .collect::<Vec<_>>();
    append_group(rows, &group);
}

fn append_groups(rows: &mut Vec<Line<'static>>, groups: &[Vec<OutputLine>]) {
    for group in groups {
        append_group(rows, group);
    }
}

fn append_group(rows: &mut Vec<Line<'static>>, group: &[OutputLine]) {
    for line in group {
        let prefix = if rows.is_empty() { "  └ " } else { "    " };
        rows.push(Line::styled(format!("{prefix}{}", line.text), line.style));
    }
}

#[cfg(test)]
#[path = "tool_render/tests.rs"]
mod tests;
