//! Unified command-session and Unix PTY integration tests.

#![cfg(unix)]

use misy_core::{ToolCall, ToolDispatcher, ToolRegistry};
use serde_json::{Value, json};
use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

fn output(result: &misy_core::ToolResult) -> Value {
    serde_json::from_str(&result.content).expect("parse command tool output")
}

async fn start(dispatcher: &ToolDispatcher, arguments: Value) -> String {
    let result = dispatcher
        .dispatch(&ToolCall::new("start", "exec_command", arguments))
        .await;
    assert!(!result.is_error, "{}", result.content);
    output(&result)["task_id"]
        .as_str()
        .expect("background task id")
        .to_owned()
}

async fn poll(dispatcher: &ToolDispatcher, task_id: &str) -> misy_core::ToolResult {
    dispatcher
        .dispatch(&ToolCall::new(
            "poll",
            "write_stdin",
            json!({"task_id": task_id, "chars": "", "yield_time_ms": 5000}),
        ))
        .await
}

async fn poll_until_terminal(dispatcher: &ToolDispatcher, task_id: &str) -> (String, Value) {
    let mut captured = String::new();
    loop {
        let result = poll(dispatcher, task_id).await;
        assert!(!result.is_error, "{}", result.content);
        let value = output(&result);
        captured.push_str(value["stdout"].as_str().unwrap_or_default());
        if !value["exit_code"].is_null() {
            return (captured, value);
        }
    }
}

fn parse_child_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("child="))?
        .trim_end_matches('\r')
        .parse()
        .ok()
}

fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_for_process_exit(pid: u32) {
    for _ in 0..40 {
        if !process_exists(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("process {pid} remained alive after PTY cleanup");
}

async fn start_pty_with_child(dispatcher: &ToolDispatcher) -> (String, u32) {
    let task_id = start(
        dispatcher,
        json!({
            "cmd": "sh -c 'trap \"\" TERM; sleep 30' & printf 'child=%s\\n' \"$!\"; wait",
            "tty": true,
            "run_in_background": true
        }),
    )
    .await;
    let mut captured = String::new();
    for _ in 0..3 {
        let result = poll(dispatcher, &task_id).await;
        captured.push_str(output(&result)["stdout"].as_str().unwrap_or_default());
        if let Some(pid) = parse_child_pid(&captured) {
            return (task_id, pid);
        }
    }
    panic!("PTY child pid was not captured: {captured:?}");
}

#[tokio::test]
async fn fast_exec_returns_inline_and_honors_the_initial_output_budget() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let result = dispatcher
        .dispatch(&ToolCall::new(
            "fast",
            "exec_command",
            json!({
                "cmd": "printf 0123456789012345678901234567890123456789",
                "max_output_tokens": 2
            }),
        ))
        .await;
    assert!(!result.is_error, "{}", result.content);
    let value = output(&result);
    assert_eq!(value["kind"], "success");
    assert!(value["task_id"].is_null());
    assert!(
        value["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("32 bytes omitted"))
    );
}

#[tokio::test]
async fn auto_yield_uses_a_bounded_command_preview_unless_description_is_given() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let preview_id = start(
        &dispatcher,
        json!({"cmd": "sleep 30\nprintf ignored", "yield_time_ms": 250}),
    )
    .await;
    let described_id = start(
        &dispatcher,
        json!({
            "cmd": "sleep 30",
            "description": "Explicit description",
            "run_in_background": true
        }),
    )
    .await;

    let listed = dispatcher
        .dispatch(&ToolCall::new("list-previews", "task_list", json!({})))
        .await;
    assert!(listed.content.contains("sleep 30"));
    assert!(listed.content.contains("Explicit description"));
    for (index, task_id) in [preview_id, described_id].iter().enumerate() {
        let stopped = dispatcher
            .dispatch(&ToolCall::new(
                format!("stop-{index}"),
                "task_stop",
                json!({"task_id": task_id}),
            ))
            .await;
        assert!(!stopped.is_error, "{}", stopped.content);
    }
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn pty_command_accepts_input_and_delivers_terminal_output_once() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let task_id = start(
        &dispatcher,
        json!({
            "cmd": "read value; printf 'got %s\\n' \"$value\"",
            "tty": true,
            "run_in_background": true
        }),
    )
    .await;

    let written = dispatcher
        .dispatch(&ToolCall::new(
            "write",
            "write_stdin",
            json!({"task_id": task_id, "chars": "hello\n", "yield_time_ms": 250}),
        ))
        .await;
    assert!(!written.is_error, "{}", written.content);
    let mut captured = output(&written)["stdout"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let written_output = output(&written);
    let terminal = if written_output["exit_code"].is_null() {
        let (remaining, terminal) = poll_until_terminal(&dispatcher, &task_id).await;
        captured.push_str(&remaining);
        terminal
    } else {
        written_output
    };

    assert!(captured.contains("got hello"), "{captured:?}");
    assert_eq!(terminal["exit_code"], 0);
    let repeated = poll(&dispatcher, &task_id).await;
    assert!(repeated.is_error);
    assert_eq!(output(&repeated)["kind"], "unknown_task");

    let listed = dispatcher
        .dispatch(&ToolCall::new("list", "task_list", json!({})))
        .await;
    assert!(listed.content.contains("completed"));
    assert!(listed.content.contains("\"exit_code\":0"));
}

#[tokio::test]
async fn pipe_session_can_be_polled_but_rejects_nonempty_input() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let task_id = start(
        &dispatcher,
        json!({
            "cmd": "sleep 0.1; printf pipe-done",
            "run_in_background": true
        }),
    )
    .await;

    let rejected = dispatcher
        .dispatch(&ToolCall::new(
            "write-pipe",
            "write_stdin",
            json!({"task_id": task_id, "chars": "input", "yield_time_ms": 250}),
        ))
        .await;
    assert!(rejected.is_error);
    assert_eq!(output(&rejected)["kind"], "stdin_closed");

    let (captured, terminal) = poll_until_terminal(&dispatcher, &task_id).await;
    assert_eq!(captured, "pipe-done");
    assert_eq!(terminal["exit_code"], 0);
}

#[tokio::test]
async fn ctrl_d_ends_pty_input_while_empty_chars_only_poll() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let task_id = start(
        &dispatcher,
        json!({"cmd": "cat; printf eof", "tty": true, "run_in_background": true}),
    )
    .await;

    let empty = dispatcher
        .dispatch(&ToolCall::new(
            "empty",
            "write_stdin",
            json!({"task_id": task_id, "chars": "", "yield_time_ms": 5000}),
        ))
        .await;
    assert!(!empty.is_error, "{}", empty.content);
    assert!(output(&empty)["exit_code"].is_null());

    let eof = dispatcher
        .dispatch(&ToolCall::new(
            "eof",
            "write_stdin",
            json!({"task_id": task_id, "chars": "\u{4}", "yield_time_ms": 250}),
        ))
        .await;
    assert!(!eof.is_error, "{}", eof.content);
    let mut captured = output(&eof)["stdout"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if output(&eof)["exit_code"].is_null() {
        captured.push_str(&poll_until_terminal(&dispatcher, &task_id).await.0);
    }
    assert!(captured.contains("eof"), "{captured:?}");
}

#[tokio::test]
async fn output_budget_omits_middle_once_and_advances_the_delivery_cursor() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let task_id = start(
        &dispatcher,
        json!({
            "cmd": "sleep 0.05; printf 0123456789012345678901234567890123456789",
            "run_in_background": true,
            "max_output_tokens": 2
        }),
    )
    .await;

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "small-poll",
            "write_stdin",
            json!({
                "task_id": task_id,
                "chars": "",
                "yield_time_ms": 5000,
                "max_output_tokens": 2
            }),
        ))
        .await;
    let value = output(&result);
    assert!(
        value["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("32 bytes omitted"))
    );
    if value["exit_code"].is_null() {
        let terminal = poll_until_terminal(&dispatcher, &task_id).await.1;
        assert_eq!(terminal["exit_code"], 0);
    }
    assert!(poll(&dispatcher, &task_id).await.is_error);
}

#[tokio::test]
async fn concurrent_polls_are_serialized_without_duplicate_delivery() {
    let dispatcher = Arc::new(ToolDispatcher::new(ToolRegistry::new()));
    let task_id = start(
        &dispatcher,
        json!({
            "cmd": "sleep 0.1; printf A; sleep 0.2; printf B",
            "run_in_background": true
        }),
    )
    .await;
    let first = tokio::spawn({
        let dispatcher = Arc::clone(&dispatcher);
        let task_id = task_id.clone();
        async move { poll(&dispatcher, &task_id).await }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second = tokio::spawn({
        let dispatcher = Arc::clone(&dispatcher);
        let task_id = task_id.clone();
        async move { poll(&dispatcher, &task_id).await }
    });

    let first = first.await.expect("first poll task");
    let second = second.await.expect("second poll task");
    assert!(!first.is_error, "{}", first.content);
    assert!(!second.is_error, "{}", second.content);
    let mut captured = format!(
        "{}{}",
        output(&first)["stdout"].as_str().unwrap_or_default(),
        output(&second)["stdout"].as_str().unwrap_or_default()
    );
    if output(&second)["exit_code"].is_null() {
        captured.push_str(&poll_until_terminal(&dispatcher, &task_id).await.0);
    }
    assert_eq!(captured, "AB");
}

#[tokio::test]
async fn task_stop_interrupts_a_long_poll_and_reaps_the_pty_group() {
    let dispatcher = Arc::new(ToolDispatcher::new(ToolRegistry::new()));
    let task_id = start(
        &dispatcher,
        json!({"cmd": "sleep 30", "tty": true, "run_in_background": true}),
    )
    .await;
    let polling = tokio::spawn({
        let dispatcher = Arc::clone(&dispatcher);
        let task_id = task_id.clone();
        async move {
            dispatcher
                .dispatch(&ToolCall::new(
                    "long-poll",
                    "write_stdin",
                    json!({"task_id": task_id, "chars": "", "yield_time_ms": 300000}),
                ))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let stopped = dispatcher
        .dispatch(&ToolCall::new(
            "stop",
            "task_stop",
            json!({"task_id": task_id}),
        ))
        .await;
    assert!(!stopped.is_error, "{}", stopped.content);
    let poll_result = tokio::time::timeout(Duration::from_secs(2), polling)
        .await
        .expect("stop must interrupt the long poll")
        .expect("poll task must not panic");
    let poll_output = output(&poll_result);
    if poll_output["exit_code"].is_null() {
        let mut status = poll_output["status"].clone();
        for _ in 0..3 {
            if status == "stopped" {
                break;
            }
            status = output(&poll(&dispatcher, &task_id).await)["status"].clone();
        }
        assert_eq!(status, "stopped");
    } else {
        assert_eq!(poll_output["status"], "stopped");
    }
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn task_stop_does_not_leave_a_pty_child_process() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let (task_id, child_pid) = start_pty_with_child(&dispatcher).await;
    assert!(process_exists(child_pid));

    let stopped = dispatcher
        .dispatch(&ToolCall::new(
            "stop-tree",
            "task_stop",
            json!({"task_id": task_id}),
        ))
        .await;
    assert!(!stopped.is_error, "{}", stopped.content);
    dispatcher.shutdown().await;

    wait_for_process_exit(child_pid).await;
}

#[tokio::test]
async fn shutdown_does_not_leave_a_pty_child_process() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let (_task_id, child_pid) = start_pty_with_child(&dispatcher).await;
    assert!(process_exists(child_pid));

    dispatcher.shutdown().await;

    wait_for_process_exit(child_pid).await;
}
