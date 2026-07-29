use super::{ToolDispatcher, ToolRegistry};
use crate::ToolCall;
use serde_json::json;
use std::time::Duration;

#[cfg(unix)]
#[tokio::test]
async fn submission_cancellation_stops_and_reaps_a_foreground_command() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let (cancel, cancellation) = tokio::sync::watch::channel(false);
    let call = ToolCall::new(
        "cancel-command",
        "exec_command",
        json!({
            "cmd": "sleep 30",
            "yield_time_ms": 30000
        }),
    );
    let dispatched = dispatcher.dispatch_cancellable(&call, cancellation);
    tokio::pin!(dispatched);
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.send_replace(true);

    let result = tokio::time::timeout(Duration::from_secs(2), dispatched)
        .await
        .expect("cancellation must finish command cleanup");
    let output: serde_json::Value = serde_json::from_str(&result.content).expect("parse output");
    assert_eq!(output["kind"], "stopped");
}

#[cfg(unix)]
#[tokio::test]
async fn submission_cancellation_interrupts_write_stdin_without_stopping_the_task() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let started = dispatcher
        .dispatch(&ToolCall::new(
            "start-observed-task",
            "exec_command",
            json!({"cmd": "sleep 30", "run_in_background": true}),
        ))
        .await;
    let started: serde_json::Value =
        serde_json::from_str(&started.content).expect("parse background task");
    let task_id = started["task_id"].as_str().expect("task id");
    let (cancel, cancellation) = tokio::sync::watch::channel(false);
    let call = ToolCall::new(
        "wait-observed-task",
        "write_stdin",
        json!({"task_id": task_id, "chars": "", "yield_time_ms": 300000}),
    );
    let output = dispatcher.dispatch_cancellable(&call, cancellation);
    tokio::pin!(output);
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.send_replace(true);

    let result = tokio::time::timeout(Duration::from_secs(2), output)
        .await
        .expect("cancellation must interrupt write_stdin");
    assert!(result.is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result.content)
            .expect("parse cancellation output")["kind"],
        "cancelled"
    );
    let tasks = dispatcher
        .dispatch(&ToolCall::new("list-observed-task", "task_list", json!({})))
        .await;
    assert!(tasks.content.contains("running"));
    dispatcher.shutdown().await;
}
