//! Headless-core integration tests.

use misy::{CoreEvent, Message, MisyCore, MisyPaths, ModelId, ModelRef, ProviderId, SubmissionId};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

#[path = "core/auth.rs"]
mod auth;
#[path = "core/limits.rs"]
mod limits;
#[path = "core/models.rs"]
mod models;
#[path = "core/queue.rs"]
mod queue;
#[path = "core/snapshots.rs"]
mod snapshots;
#[path = "core/streaming.rs"]
mod streaming;

fn write_fixture_manifest(root: &Path, id: &str, fixture: &Path, target: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "display_name": "{id} fixture",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 2,
  "description": "Core fixture",
  "capabilities": {{"usage": {{"version": 1}}}},
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{}",
  "args": ["{}"],
  "auth_methods": [{{"id": "oauth", "display_name": "Fixture OAuth"}}]
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

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "fixture did not create {}", path.display());
}

fn fixture_signal(target: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{suffix}", target.display()))
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
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
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
        .complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
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
fn usage_routes_to_the_models_provider_and_rejects_raw_provider_payloads() {
    let (_temporary, core, _) = test_core("usage");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");

    let report = core.usage(&fixture_model()).expect("usage report");

    assert_eq!(report.limits[0].id, "five-hour");
    assert_eq!(report.limits[0].amount.used, Some(42.0));
    core.shutdown().expect("shutdown");

    let (_temporary, core, _) = test_core("bad-usage");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");
    let error = core
        .usage(&fixture_model())
        .expect_err("raw usage payload must be rejected")
        .to_string();
    assert!(error.contains("unknown field `raw`"));
    assert!(!error.contains("must-not-escape"));
    core.shutdown().expect("shutdown");
}

#[test]
fn credential_method_is_read_locally_without_starting_a_provider() {
    let (_temporary, core, _) = test_core("credential-method");
    let provider = ProviderId::new("fixture");

    assert_eq!(core.credential_method(&provider).expect("method"), None);
    assert_eq!(core.running_provider_count(), 0);

    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");
    core.shutdown().expect("shutdown");

    assert_eq!(
        core.credential_method(&provider).expect("method"),
        Some("oauth".to_owned())
    );
}

#[test]
fn available_models_skips_unconfigured_providers_and_isolates_provider_failures() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("models.txt"),
    );
    write_fixture_manifest(
        &bundled,
        "failed-provider",
        &fixture,
        &temporary.path().join("bad-models.txt"),
    );
    write_fixture_manifest(
        &bundled,
        "unconfigured-provider",
        &fixture,
        &temporary.path().join("unconfigured.txt"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");

    for provider in [
        ProviderId::new("fixture"),
        ProviderId::new("failed-provider"),
    ] {
        core.complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .expect("authenticate provider");
    }
    let available = core.available_models().expect("available models");

    assert_eq!(available.models.len(), 2);
    assert_eq!(available.errors.len(), 1);
    assert_eq!(available.errors[0].provider.as_str(), "failed-provider");
    assert_eq!(
        available.errors[0].provider_display_name,
        "failed-provider fixture"
    );
    assert_eq!(core.running_provider_count(), 2);
    core.shutdown().expect("shutdown");
}

#[test]
fn cached_available_models_returns_models_without_provider_processes() {
    let (temporary, core, _) = test_core("cached-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");
    let expected = core.list_models(&provider).expect("list models");
    core.shutdown().expect("shutdown first core");

    let cached_core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        temporary.path().join("bundled"),
    )
    .expect("reopen core");

    assert_eq!(
        cached_core
            .cached_available_models()
            .expect("cached available models")
            .models,
        expected
    );
    assert_eq!(cached_core.running_provider_count(), 0);
    cached_core.shutdown().expect("shutdown cached core");
}

#[test]
fn logout_removes_the_provider_model_cache() {
    let (_temporary, core, _) = test_core("logout-model-cache");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");
    core.list_models(&provider).expect("list models");
    assert_eq!(
        core.cached_available_models()
            .expect("cached models")
            .models
            .len(),
        2
    );

    core.logout(&provider).expect("logout");

    assert!(
        core.cached_available_models()
            .expect("cached models after logout")
            .models
            .is_empty()
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn late_model_response_cannot_restore_cache_after_logout() {
    let (_temporary, core, _) = test_core("slow-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");

    let listing = {
        let core = core.clone();
        let provider = provider.clone();
        std::thread::spawn(move || core.list_models(&provider))
    };
    std::thread::sleep(Duration::from_millis(50));
    core.logout(&provider).expect("logout while listing models");
    listing
        .join()
        .expect("model-listing thread")
        .expect("model listing");

    assert!(
        core.cached_available_models()
            .expect("cached models after logout")
            .models
            .is_empty()
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn pre_logout_model_response_cannot_populate_a_reauthenticated_catalog() {
    let (temporary, core, target) = test_core("async-gated-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "first"}),
    )
    .expect("first authentication");

    let listing = {
        let core = core.clone();
        let provider = provider.clone();
        std::thread::spawn(move || core.list_models(&provider))
    };
    let started = fixture_signal(&target, "started");
    wait_for_file(&started);
    core.logout(&provider).expect("logout after request starts");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "second"}),
    )
    .expect("second authentication");
    fs::write(fixture_signal(&target, "release"), "release stale response")
        .expect("release stale response");
    listing
        .join()
        .expect("model-listing thread")
        .expect("delayed model listing");

    assert!(
        core.cached_available_models()
            .expect("cached models after reauthentication")
            .models
            .is_empty()
    );
    core.shutdown().expect("shutdown");
    drop(temporary);
}

#[test]
fn core_passes_stored_credentials_to_streaming_chat_requests() {
    let (_temporary, core, _) = test_core("chat-credentials");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("authenticate");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core
        .submit(Message::user("credential-chat"))
        .expect("submit");
    let received = receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    );
    assert!(received.iter().any(|event| {
        matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "authenticated")
    }));
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
fn core_recursively_sanitizes_successful_auth_results_after_persisting_top_level_credentials() {
    let (temporary, core, _) = test_core("nested-auth");
    let provider = ProviderId::new("fixture");

    for response in [
        core.auth_status(&provider).expect("status"),
        core.start_auth(&provider).expect("start"),
        core.complete_auth(
            &provider,
            json!({"id":"fixture-session"}),
            json!({"code":"nested"}),
        )
        .expect("complete"),
        core.refresh_auth(&provider).expect("refresh"),
    ] {
        assert!(
            !serde_json::to_string(&response)
                .expect("public response")
                .contains("secret")
        );
    }
    let credentials = fs::read_to_string(temporary.path().join("misy/credentials.json"))
        .expect("stored credentials");
    assert!(credentials.contains("refreshed-opaque"));
    core.shutdown().expect("shutdown");
}

#[test]
fn core_sanitizes_credentials_from_remote_auth_errors() {
    let (_temporary, core, _) = test_core("remote-auth-error");
    let provider = ProviderId::new("fixture");
    let error = core
        .complete_auth(&provider, json!({"id":"remote-error"}), json!({}))
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
    assert_eq!(history[0].message.content, "session-one");
    assert_eq!(history[2].message.content, "session-two");
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
    let first_for_thread = first;
    let second_for_thread = second;

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
        std::thread::spawn(move || {
            core.complete_auth(
                &provider,
                json!({"id":"fixture-session"}),
                json!({"code":"a"}),
            )
        })
    };
    let complete_b = {
        let core = core.clone();
        let provider = second.clone();
        std::thread::spawn(move || {
            core.complete_auth(
                &provider,
                json!({"id":"fixture-session"}),
                json!({"code":"b"}),
            )
        })
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
fn pending_auth_for_one_provider_does_not_block_logout_for_another() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("first"),
    );
    write_fixture_manifest(
        &bundled,
        "fixture-two",
        &fixture,
        &temporary.path().join("second"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    let first = ProviderId::new("fixture");
    let second = ProviderId::new("fixture-two");
    core.complete_auth(
        &second,
        json!({"id":"fixture-session"}),
        json!({"code":"b"}),
    )
    .expect("authenticate second provider");

    let pending = {
        let core = core.clone();
        let provider = first;
        std::thread::spawn(move || {
            core.complete_auth(&provider, json!({"id":"pending-a"}), json!({}))
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    core.logout(&second).expect("logout second provider");
    assert!(started.elapsed() < Duration::from_millis(500));

    pending.join().expect("pending auth thread").expect("auth");
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
