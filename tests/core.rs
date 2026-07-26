use misy::{CoreEvent, Message, MisyCore, MisyPaths, ModelId, ModelRef, ProviderId, SubmissionId};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

fn write_fixture_manifest(root: &Path, fixture: &Path, target: &Path) {
    let package = root.join("fixture");
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "fixture",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 1,
  "description": "Core fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{}",
  "args": ["{}"]
}}"#,
            fixture.display(),
            target.display(),
        ),
    )
    .expect("manifest");
}

fn test_core(name: &str) -> (tempfile::TempDir, MisyCore, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join(format!("{name}.txt"));
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, &fixture, &target);
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    (temporary, core, target)
}

fn fixture_model() -> ModelRef {
    ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model"))
}

fn receive_until(
    events: &Receiver<CoreEvent>,
    submission: SubmissionId,
    predicate: impl Fn(&CoreEvent) -> bool,
) -> Vec<CoreEvent> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = Vec::new();
    while Instant::now() < deadline {
        let event = events
            .recv_timeout(Duration::from_millis(100))
            .expect("core event before timeout");
        let done = matches!(
            event,
            CoreEvent::Completed { submission: id } | CoreEvent::Cancelled { submission: id }
                if id == submission
        ) || matches!(&event, CoreEvent::Failed { submission: id, .. } if *id == submission);
        let matches_predicate = predicate(&event);
        received.push(event);
        if matches_predicate || done {
            return received;
        }
    }
    panic!("did not receive expected core event");
}

#[test]
fn core_discovers_authenticates_lists_and_persists_the_selected_model() {
    let (temporary, core, _) = test_core("setup");
    let provider = ProviderId::new("fixture");

    assert_eq!(core.providers().len(), 1);
    assert_eq!(
        core.auth_status(&provider).expect("auth status")["authenticated"],
        false
    );
    assert_eq!(
        core.start_auth(&provider).expect("auth start")["url"],
        "https://example.test/auth"
    );
    core.complete_auth(&provider, json!({"code": "opaque"}))
        .expect("auth complete");

    assert_eq!(
        core.list_models(&provider).expect("models")[0].model,
        fixture_model()
    );
    core.select_model(fixture_model()).expect("select model");
    assert_eq!(core.selected_model(), Some(fixture_model()));
    drop(core);

    let reopened = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        temporary.path().join("bundled"),
    )
    .expect("reopen core");
    assert_eq!(reopened.selected_model(), Some(fixture_model()));
    reopened.shutdown().expect("shutdown");
}

#[test]
fn core_streams_a_tool_round_trip_and_keeps_provider_and_model_on_every_turn() {
    let (_temporary, core, target) = test_core("tool-round-trip");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core
        .submit(Message::user("tool-round-trip"))
        .expect("submit");

    let received = receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    );
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
    assert!(core.history().iter().any(|entry| {
        entry.message.role == misy::MessageRole::Assistant
            && entry.provider_metadata["response"] == json!({"turn": "one"})
            && entry.provider_metadata["stream"].as_array().is_some()
    }));
    core.shutdown().expect("shutdown");
}

#[test]
fn core_returns_tool_errors_to_the_provider_for_malformed_and_unknown_calls() {
    for prompt in ["bad-tool-arguments", "unknown-tool"] {
        let (_temporary, core, _) = test_core(prompt);
        core.select_model(fixture_model()).expect("select model");
        let events = core.subscribe();
        let submission = core.submit(Message::user(prompt)).expect("submit");
        let received = receive_until(
            &events,
            submission,
            |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
        );
        assert!(
            received.iter().any(
                |event| matches!(event, CoreEvent::ToolResult { result, .. } if result.is_error)
            )
        );
        core.shutdown().expect("shutdown");
    }
}

#[test]
fn core_emits_provider_failure_and_honours_cancellation() {
    let (_temporary, core, _) = test_core("failure-cancel");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();

    let failed = core
        .submit(Message::user("provider-failure"))
        .expect("submit");
    let received = receive_until(
        &events,
        failed,
        |event| matches!(event, CoreEvent::Failed { submission, .. } if *submission == failed),
    );
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::Failed { .. }))
    );

    let cancelled = core.submit(Message::user("cancel-me")).expect("submit");
    core.cancel(cancelled).expect("cancel");
    let received = receive_until(
        &events,
        cancelled,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == cancelled),
    );
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::Cancelled { .. }))
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn core_stops_after_sixty_four_model_turns() {
    let (_temporary, core, _) = test_core("turn-limit");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core.submit(Message::user("turn-limit")).expect("submit");

    let received = receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Failed { submission: id, message } if *id == submission && message.contains("64")),
    );
    assert!(
        received.iter().any(
            |event| matches!(event, CoreEvent::Failed { message, .. } if message.contains("64"))
        )
    );
    core.shutdown().expect("shutdown");
}
