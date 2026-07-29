//! Semantic rendering for committed transcript rows.

use super::{state::TranscriptRow, style};
use ratatui::text::{Line, Span};

/// Converts transcript rows into styled terminal lines.
pub(super) fn transcript_lines(rows: &[TranscriptRow]) -> Vec<Line<'static>> {
    rows.iter().flat_map(row_lines).collect()
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
        TranscriptRow::ActivityFinished(output) => activity_finished_lines(output),
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
    let summary = command_call_summary(name, arguments);
    Line::from(vec![
        Span::styled("⏺ ", style::accent()),
        Span::raw(summary.unwrap_or_else(|| format!("{name}({})", arguments.unwrap_or_default()))),
    ])
}

fn command_call_summary(name: &str, arguments: Option<&str>) -> Option<String> {
    if name != "exec_command" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(arguments?).ok()?;
    let description = arguments
        .get("description")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let command =
        description.or_else(|| arguments.get("cmd").and_then(serde_json::Value::as_str))?;
    Some(format!("{name} · {command}"))
}

fn tool_result_lines(is_error: bool, content: Option<&str>) -> Vec<Line<'static>> {
    if let Some(lines) = command_result_lines(content) {
        return lines;
    }
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

fn command_result_lines(content: Option<&str>) -> Option<Vec<Line<'static>>> {
    let value: serde_json::Value = serde_json::from_str(content?).ok()?;
    let kind = value.get("kind")?.as_str()?;
    if kind == "background" {
        let id = value.get("task_id")?.as_str()?;
        return Some(vec![Line::styled(
            format!("  ⎿ Running in background · {id}"),
            style::muted(),
        )]);
    }
    None
}

fn activity_finished_lines(output: &misy_core::ActivityOutput) -> Vec<Line<'static>> {
    let (marker, state, header_style) = match output.activity.status {
        misy_core::ActivityStatus::Completed => ("●", "completed", style::success()),
        misy_core::ActivityStatus::Failed => ("×", "failed", style::error()),
        misy_core::ActivityStatus::Stopped => ("■", "stopped", style::muted()),
        misy_core::ActivityStatus::Queued
        | misy_core::ActivityStatus::Running
        | misy_core::ActivityStatus::Waiting => ("○", "finished", style::muted()),
    };
    let mut lines = vec![Line::styled(
        format!(
            "{marker} Background task {state} · {} · {}",
            output.activity.id, output.activity.title
        ),
        header_style,
    )];
    append_activity_stream(&mut lines, &output.stdout, "");
    append_activity_stream(&mut lines, &output.stderr, "stderr: ");
    if output.stdout.is_empty() && output.stderr.is_empty() {
        lines.push(Line::styled("  ⎿ (no output)", style::muted()));
    }
    if let Some(message) = &output.message {
        lines.push(Line::styled(format!("  ⎿ {message}"), header_style));
    }
    if let Some(code) = output.activity.exit_code {
        lines.push(Line::styled(
            format!("  ⎿ Process exited with code {code}"),
            if code == 0 {
                style::muted()
            } else {
                style::error()
            },
        ));
    }
    lines
}

fn append_activity_stream(lines: &mut Vec<Line<'static>>, content: &str, label: &str) {
    for (index, line) in content.lines().enumerate() {
        let prefix = if index == 0 { "  ⎿ " } else { "    " };
        lines.push(Line::styled(
            format!("{prefix}{label}{line}"),
            style::muted(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{activity_finished_lines, command_call_summary, command_result_lines};
    use misy_core::ActivityOutput;

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
    fn background_start_result_hides_internal_json() {
        let content = serde_json::json!({
            "kind": "background",
            "task_id": "task-4",
            "status": "running",
            "stdout": ""
        })
        .to_string();
        let lines = command_result_lines(Some(&content)).expect("command result");
        assert_eq!(text(&lines), ["  ⎿ Running in background · task-4"]);
    }

    #[test]
    fn exec_call_uses_description_instead_of_argument_json() {
        let arguments = serde_json::json!({
            "cmd": "for i in $(seq 1 60); do echo $i; done",
            "description": "Print one number per second"
        })
        .to_string();
        assert_eq!(
            command_call_summary("exec_command", Some(&arguments)).as_deref(),
            Some("exec_command · Print one number per second")
        );
    }

    #[test]
    fn completion_renders_header_output_and_exit_status() {
        let output: ActivityOutput = serde_json::from_value(serde_json::json!({
            "activity": {
                "id": 4,
                "kind": "task",
                "status": "completed",
                "title": "Print numbers",
                "cwd": null,
                "started_at_ms": 1,
                "exit_code": 0
            },
            "stdout": "one\ntwo\n",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "message": null
        }))
        .expect("deserialize activity output fixture");
        assert_eq!(
            text(&activity_finished_lines(&output)),
            [
                "● Background task completed · task-4 · Print numbers",
                "  ⎿ one",
                "    two",
                "  ⎿ Process exited with code 0"
            ]
        );
    }
}
