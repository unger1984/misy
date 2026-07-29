use super::{friendly_header, tool_blocks};
use crate::tui::state::TranscriptRow;

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

fn call(id: &str, name: &str, arguments: &serde_json::Value) -> TranscriptRow {
    TranscriptRow::ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: Some(arguments.to_string()),
    }
}

fn result(id: &str, content: String) -> TranscriptRow {
    TranscriptRow::ToolResult {
        id: id.to_owned(),
        is_error: false,
        content: Some(content),
    }
}

#[test]
fn all_builtin_headers_hide_noisy_arguments() {
    let cases = [
        (
            "list_directory",
            serde_json::json!({"path": "src"}),
            "List src",
        ),
        (
            "read_file",
            serde_json::json!({"path": "README.md"}),
            "Read README.md",
        ),
        (
            "view_image",
            serde_json::json!({"path": "shot.png"}),
            "View image shot.png",
        ),
        (
            "exec_command",
            serde_json::json!({"cmd": "cargo test"}),
            "Run cargo test",
        ),
        (
            "write_file",
            serde_json::json!({"path": "out.txt", "content": "secret"}),
            "Write out.txt",
        ),
        ("task_list", serde_json::json!({}), "List tasks"),
        (
            "task_stop",
            serde_json::json!({"task_id": "task-2"}),
            "Stop task task-2",
        ),
        (
            "write_stdin",
            serde_json::json!({"task_id": "task-2"}),
            "Poll task task-2",
        ),
        (
            "write_stdin",
            serde_json::json!({"task_id": "task-2", "chars": ""}),
            "Poll task task-2",
        ),
        (
            "write_stdin",
            serde_json::json!({"task_id": "task-2", "chars": "secret"}),
            "Send input task-2",
        ),
    ];
    for (name, arguments, expected) in cases {
        assert_eq!(
            friendly_header(name, Some(&arguments.to_string()), 80),
            expected
        );
    }
}

#[test]
fn result_pairs_with_latest_preceding_unmatched_call() {
    let rows = vec![
        call("same", "read_file", &serde_json::json!({"path": "first"})),
        call("same", "read_file", &serde_json::json!({"path": "second"})),
        result("same", "second-result".to_owned()),
    ];
    let blocks = tool_blocks(&rows, 80, false, "Ctrl+O");

    assert_eq!(text(&blocks[&0]), ["○ Read first"]);
    assert_eq!(text(&blocks[&1]), ["● Read second", "  └ second-result"]);
    assert!(!blocks.contains_key(&2));
}

#[test]
fn error_and_orphan_results_use_failure_and_fallback_headers() {
    let rows = vec![
        TranscriptRow::ToolCall {
            id: "known".to_owned(),
            name: "plugin_tool".to_owned(),
            arguments: Some("{\"value\":42}".to_owned()),
        },
        TranscriptRow::ToolResult {
            id: "known".to_owned(),
            is_error: true,
            content: Some("boom".to_owned()),
        },
        result("orphan", "late".to_owned()),
    ];
    let blocks = tool_blocks(&rows, 80, false, "Ctrl+O");

    assert_eq!(
        text(&blocks[&0]),
        ["× Called plugin_tool({\"value\":42})", "  └ boom"]
    );
    assert_eq!(text(&blocks[&2]), ["● Tool completed", "  └ late"]);
}

#[test]
fn compact_and_expanded_budgets_include_the_marker() {
    let output = (1..=30)
        .map(|number| format!("line-{number:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = vec![
        call("one", "read_file", &serde_json::json!({"path": "long"})),
        result("one", output),
    ];

    let compact = text(&tool_blocks(&rows, 80, false, "Alt+E")[&0]);
    let expanded = text(&tool_blocks(&rows, 80, true, "Alt+E")[&0]);
    assert_eq!(compact.len(), 5);
    assert_eq!(expanded.len(), 13);
    assert!(
        compact
            .iter()
            .any(|line| line.contains("… 27 more lines (Alt+E to expand)"))
    );
    assert!(expanded.iter().any(|line| line.contains("… 19 more lines")));
    assert!(!expanded.iter().any(|line| line.contains("to expand")));
    assert!(compact.iter().any(|line| line.contains("line-01")));
    assert!(compact.iter().any(|line| line.contains("line-30")));
}

#[test]
fn wrapped_output_recomputes_exact_hidden_logical_lines() {
    let output = (1..=30)
        .map(|number| format!("logical-{number:02}-with-wide-content"))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = vec![
        call("one", "read_file", &serde_json::json!({"path": "long"})),
        result("one", output),
    ];
    let wide = text(&tool_blocks(&rows, 80, false, "Ctrl+O")[&0]);
    let narrow = text(&tool_blocks(&rows, 18, false, "Ctrl+O")[&0]);

    assert!(wide.iter().any(|line| line.contains("… 27 more lines")));
    let narrow_marker = narrow[1..].concat().replace("    ", "");
    assert!(narrow_marker.contains("… 30 more lines"), "{narrow:?}");
}

#[test]
fn ultra_narrow_output_falls_back_to_marker_only() {
    let rows = vec![
        call("one", "read_file", &serde_json::json!({"path": "long"})),
        result("one", "one\ntwo\nthree\nfour\nfive".to_owned()),
    ];
    let rendered = text(&tool_blocks(&rows, 5, false, "Ctrl+O")[&0]);
    let output = &rendered[1..];

    let joined = output.concat().replace(' ', "");
    assert!(joined.contains("…5morelines(Ctrl+Otoexpand)"), "{joined:?}");
    assert!(!output.iter().any(|line| line.contains("one")));
}

#[test]
fn wide_unicode_is_safe_when_only_one_content_cell_is_available() {
    let rows = vec![
        call("one", "read_file", &serde_json::json!({"path": "wide"})),
        result("one", "界界".to_owned()),
    ];
    let rendered = tool_blocks(&rows, 5, false, "Ctrl+O");

    assert!(rendered[&0][1..].iter().all(|line| line.width() <= 5));
}

#[test]
fn background_start_hides_internal_json() {
    let content = serde_json::json!({
        "kind": "background",
        "task_id": "task-4",
        "status": "running",
        "stdout": ""
    })
    .to_string();
    let rows = vec![
        call(
            "one",
            "exec_command",
            &serde_json::json!({"cmd": "sleep 5"}),
        ),
        result("one", content),
    ];
    let rendered = text(&tool_blocks(&rows, 80, false, "Ctrl+O")[&0]);

    assert_eq!(
        rendered,
        ["● Run sleep 5", "  └ Running in background · task-4"]
    );
}

#[test]
fn empty_result_has_an_explicit_placeholder() {
    let rows = vec![
        call("one", "task_list", &serde_json::json!({})),
        result("one", String::new()),
    ];

    assert_eq!(
        text(&tool_blocks(&rows, 80, false, "Ctrl+O")[&0]),
        ["● List tasks", "  └ (no output)"]
    );
}

#[test]
fn terminal_activity_uses_ordered_fragments_and_the_same_budget() {
    let output: misy_core::ActivityOutput = serde_json::from_value(serde_json::json!({
        "activity": {
            "id": 4, "kind": "task", "status": "completed", "title": "Build",
            "cwd": null, "started_at_ms": 1, "exit_code": 0
        },
        "stdout": "beforeafter", "stderr": "warning", "stdout_truncated": false,
        "stderr_truncated": false,
        "fragments": [
            {"stream": "stdout", "text": "before"},
            {"stream": "stderr", "text": "warning"},
            {"stream": "stdout", "text": "after"}
        ],
        "message": null
    }))
    .expect("activity fixture");
    let rows = vec![TranscriptRow::ActivityFinished(output)];
    let rendered = text(&tool_blocks(&rows, 80, false, "Ctrl+O")[&0]);

    assert_eq!(rendered.len(), 5);
    assert_eq!(rendered[0], "● Background task completed · task-4 · Build");
    assert!(rendered.iter().any(|line| line.contains("stderr: warning")));
    assert!(rendered.iter().any(|line| line.contains("Process exited")));
}
