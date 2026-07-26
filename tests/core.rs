use misy::{CoreEvent, Message, MisyCore, MisyPaths, ModelId, ModelRef, ProviderId, SubmissionId};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

fn write_fixture_manifest(root: &Path, id: &str, fixture: &Path, target: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
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
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
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
        let Ok(event) = events.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
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
fn core_persists_auth_credentials_without_exposing_them_to_callers() {
    let (temporary, core, _) = test_core("redacted-auth");
    let provider = ProviderId::new("fixture");

    let completed = core
        .complete_auth(&provider, json!({"code": "opaque"}))
        .expect("auth complete");

    assert!(completed.get("credentials").is_none());
    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("stored credentials")
            .contains("opaque")
    );
    let refreshed = core.refresh_auth(&provider).expect("auth refresh");
    assert!(refreshed.get("credentials").is_none());
    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("refreshed credentials")
            .contains("refreshed-opaque")
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn core_sanitizes_all_public_auth_responses() {
    let (_temporary, core, _) = test_core("sanitized-auth");
    let provider = ProviderId::new("fixture");

    assert!(
        core.auth_status(&provider)
            .expect("status")
            .get("credentials")
            .is_none()
    );
    assert!(
        core.start_auth(&provider)
            .expect("start")
            .get("credentials")
            .is_none()
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn core_sanitizes_credentials_from_remote_auth_errors() {
    let (_temporary, core, _) = test_core("remote-auth-error");
    let provider = ProviderId::new("fixture");
    let error = core
        .complete_auth(&provider, json!({"code":"remote-error"}))
        .expect_err("remote auth failure");
    let misy::CoreError::Provider(misy::ProviderError::Remote { data, .. }) = error else {
        panic!("expected remote provider error");
    };
    assert!(
        !serde_json::to_string(&data)
            .expect("error data")
            .contains("secret")
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn subscription_replays_the_discovered_provider_snapshot() {
    let (_temporary, core, _) = test_core("provider-snapshot");
    let events = core.subscribe();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).expect("provider snapshot"),
        CoreEvent::ProviderDiscovered { provider } if provider.as_str() == "fixture"
    ));
    core.shutdown().expect("shutdown");
}

#[test]
fn subscription_snapshot_includes_more_than_the_default_event_buffer() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("target.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    for index in 0..129 {
        write_fixture_manifest(&bundled, &format!("fixture-{index}"), &fixture, &target);
    }
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    let events = core.subscribe();
    let mut providers = std::collections::BTreeSet::new();
    for _ in 0..129 {
        let CoreEvent::ProviderDiscovered { provider } = events
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot event")
        else {
            panic!("only provider snapshot events are expected");
        };
        providers.insert(provider.as_str().to_owned());
    }
    assert_eq!(providers.len(), 129);
    core.shutdown().expect("shutdown");
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
fn core_losslessly_collects_a_burst_of_provider_stream_events() {
    let (_temporary, core, _) = test_core("burst");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core.submit(Message::user("burst")).expect("submit");

    receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    );
    let assistant = core
        .history()
        .into_iter()
        .find(|entry| entry.message.role == misy::MessageRole::Assistant)
        .expect("assistant history");
    assert_eq!(assistant.message.content.len(), 4_096);
    assert_eq!(assistant.message.content, "x".repeat(4_096));
    core.shutdown().expect("shutdown");
}

#[test]
fn core_serializes_concurrent_submissions_into_one_canonical_history() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("target.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    write_fixture_manifest(&bundled, "fixture-two", &fixture, &target);
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    core.select_model(fixture_model())
        .expect("select first model");
    let events = core.subscribe();
    let first = core
        .submit(Message::user("session-one"))
        .expect("first submit");
    core.select_model(ModelRef::new(
        ProviderId::new("fixture-two"),
        ModelId::new("fixture-model"),
    ))
    .expect("select second model");
    let second = core
        .submit(Message::user("session-two"))
        .expect("second submit");

    receive_until(
        &events,
        first,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == first),
    );
    receive_until(
        &events,
        second,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == second),
    );
    let history = core.history();
    assert_eq!(history.len(), 4);
    assert!(history.chunks_exact(2).all(|pair| {
        pair[0].message.role == misy::MessageRole::User
            && pair[1].message.role == misy::MessageRole::Assistant
    }));
    core.shutdown().expect("shutdown");
}

#[test]
fn concurrent_model_selection_keeps_memory_and_disk_in_sync() {
    let (temporary, core, _) = test_core("model-race");
    let first = fixture_model();
    let second = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));
    let core_a = core.clone();
    let core_b = core.clone();
    let first_for_thread = first.clone();
    let second_for_thread = second.clone();

    let select_a = std::thread::spawn(move || core_a.select_model(first_for_thread));
    let select_b = std::thread::spawn(move || core_b.select_model(second_for_thread));
    select_a
        .join()
        .expect("first selector")
        .expect("first selection");
    select_b
        .join()
        .expect("second selector")
        .expect("second selection");

    let persisted = misy::ConfigStore::new(MisyPaths::from_root(temporary.path().join("misy")))
        .load()
        .expect("saved config")
        .default_model;
    assert_eq!(core.selected_model(), persisted);
    core.shutdown().expect("shutdown");
}

#[test]
fn concurrent_auth_mutations_do_not_lose_or_resurrect_credentials() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("target.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    write_fixture_manifest(&bundled, "fixture-two", &fixture, &target);
    let paths = MisyPaths::from_root(temporary.path().join("misy"));
    let core = MisyCore::discover(paths.clone(), &bundled).expect("core discovery");
    let first = ProviderId::new("fixture");
    let second = ProviderId::new("fixture-two");

    let complete_a = {
        let core = core.clone();
        let provider = first.clone();
        std::thread::spawn(move || core.complete_auth(&provider, json!({"code":"a"})))
    };
    let complete_b = {
        let core = core.clone();
        let provider = second.clone();
        std::thread::spawn(move || core.complete_auth(&provider, json!({"code":"b"})))
    };
    complete_a
        .join()
        .expect("first completion")
        .expect("first auth");
    complete_b
        .join()
        .expect("second completion")
        .expect("second auth");
    let store = misy::CredentialStore::new(paths);
    assert!(store.load(&first).expect("first credential").is_some());
    assert!(store.load(&second).expect("second credential").is_some());

    let refreshing = {
        let core = core.clone();
        let provider = first.clone();
        std::thread::spawn(move || core.refresh_auth(&provider))
    };
    std::thread::sleep(Duration::from_millis(50));
    let logging_out = {
        let core = core.clone();
        let provider = first.clone();
        std::thread::spawn(move || core.logout(&provider))
    };
    refreshing.join().expect("refresh thread").expect("refresh");
    logging_out.join().expect("logout thread").expect("logout");
    assert!(store.load(&first).expect("logged out credential").is_none());
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
fn cancellation_before_tool_dispatch_prevents_tool_side_effects() {
    let (_temporary, core, target) = test_core("cancel-before-tool");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core
        .submit(Message::user("cancel-before-tool"))
        .expect("submit");

    receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::ToolCall { submission: id, .. } if *id == submission),
    );
    core.cancel(submission).expect("cancel");
    receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Cancelled { submission: id } if *id == submission),
    );
    assert!(!target.exists(), "cancelled tool must not write a file");
    core.shutdown().expect("shutdown");
}

#[test]
fn cancelling_a_submission_queued_on_the_session_gate_does_not_append_its_prompt() {
    let (_temporary, core, _) = test_core("queued-cancel");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let blocking = core
        .submit(Message::user("block-session"))
        .expect("blocking submit");
    receive_until(
        &events,
        blocking,
        |event| matches!(event, CoreEvent::SubmissionStarted { submission, .. } if *submission == blocking),
    );
    let queued = core
        .submit(Message::user("queued-cancel"))
        .expect("queued submit");
    core.cancel(queued).expect("cancel queued submission");

    receive_until(
        &events,
        blocking,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == blocking),
    );
    receive_until(
        &events,
        queued,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == queued),
    );
    assert!(
        !core
            .history()
            .iter()
            .any(|entry| entry.message.content == "queued-cancel")
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn late_stream_events_from_a_cancelled_request_do_not_reach_the_next_request() {
    let (_temporary, core, _) = test_core("late-events");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let cancelled = core
        .submit(Message::user("late-cancel"))
        .expect("cancelled submission");
    receive_until(
        &events,
        cancelled,
        |event| matches!(event, CoreEvent::SubmissionStarted { submission, .. } if *submission == cancelled),
    );
    core.cancel(cancelled).expect("cancel");
    receive_until(
        &events,
        cancelled,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == cancelled),
    );

    let next = core
        .submit(Message::user("late-next"))
        .expect("next submission");
    let received = receive_until(
        &events,
        next,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == next),
    );
    assert!(received.iter().any(
        |event| matches!(event, CoreEvent::TextDelta { delta, submission, .. } if *submission == next && delta == "next")
    ));
    assert!(!received.iter().any(
        |event| matches!(event, CoreEvent::TextDelta { delta, submission, .. } if *submission == next && delta == "late")
    ));
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
