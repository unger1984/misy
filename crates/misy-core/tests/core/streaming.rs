//! Streaming and tool-round-trip coverage for the headless core.

use super::{fixture_model, receive_until, test_core, test_core_with_deadlines};
use misy_core::{CoreEvent, Message, MessageRole, ProviderDeadlines};
use serde_json::json;
use std::{fs, time::Duration};

#[tokio::test]
async fn core_streams_a_tool_round_trip_and_keeps_provider_and_model_on_every_turn() {
    let (_temporary, core, target) = test_core("tool-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("tool-round-trip"))
        .await
        .expect("submit");

    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "writing"))
    );
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::ToolResult { result, .. } if !result.is_error))
    );
    let tool_results: Vec<_> = received
        .iter()
        .filter_map(|event| match event {
            CoreEvent::ToolResult { result, .. } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_results
            .iter()
            .map(|result| result.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        ["write-1", "read-1"]
    );
    assert!(tool_results.iter().all(|result| !result.is_error));
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "done"))
    );
    assert_eq!(
        fs::read_to_string(target).expect("tool output"),
        "written by tool"
    );
    assert!(core.history().await.iter().any(|entry| {
        entry.message.role == MessageRole::Assistant
            && entry.provider_metadata["response"] == json!({"turn": "one"})
            && entry.provider_metadata["stream"].as_array().is_some()
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_losslessly_collects_a_burst_of_provider_stream_events() {
    let (_temporary, core, _) = test_core("burst");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core.submit(Message::user("burst")).await.expect("submit");

    receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    let assistant = core
        .history()
        .await
        .into_iter()
        .find(|entry| entry.message.role == MessageRole::Assistant)
        .expect("assistant history");
    assert_eq!(assistant.message.content.len(), 4_096);
    assert_eq!(assistant.message.content, "x".repeat(4_096));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_fails_a_stream_that_goes_idle_and_restarts_the_provider() {
    let deadlines = ProviderDeadlines {
        stream_idle: Duration::from_millis(200),
        ..ProviderDeadlines::default()
    };
    let (_temporary, core, _) = test_core_with_deadlines("stream-idle", deadlines);
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();

    // The fixture answers `cancel-me` without ever completing the stream, so the turn goes
    // silent right after the response and must hit the idle deadline instead of hanging.
    let hung = core
        .submit(Message::user("cancel-me"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        hung,
        |event| matches!(event, CoreEvent::Failed { submission, .. } if *submission == hung),
    )
    .await;
    assert!(received.iter().any(|event| {
        matches!(
            event,
            CoreEvent::Failed { message, .. } if message.contains("no stream activity")
        )
    }));

    // The hung process was reaped, so a fresh submission reaches a restarted provider.
    let recovered = core
        .submit(Message::user("session-one"))
        .await
        .expect("submit after idle failure");
    receive_until(
        &mut events,
        recovered,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == recovered),
    )
    .await;
    core.shutdown().await.expect("shutdown");
}
