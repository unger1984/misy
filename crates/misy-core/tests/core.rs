//! Headless-core integration tests.

use misy_core::{
    ClientCapabilities, CoreEvent, CoreOptions, ImageAttachment, InputModality, Message, MisyCore,
    MisyPaths, ModelId, ModelRef, ProviderDeadlines, ProviderId, SubmissionId,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedReceiver;

#[path = "core/agents.rs"]
mod agents;
#[path = "core/auth.rs"]
mod auth;
#[path = "core/basic.rs"]
mod basic;
#[path = "core/cache_integration.rs"]
mod cache_integration;
#[path = "core/concurrency.rs"]
mod concurrency;
#[path = "core/contracts_integration.rs"]
mod contracts_integration;
#[path = "core/images.rs"]
mod images;
#[path = "core/instructions.rs"]
mod instructions;
#[path = "core/lifecycle.rs"]
mod lifecycle;
#[path = "core/limits.rs"]
mod limits;
#[path = "core/models.rs"]
mod models;
#[path = "core/questions.rs"]
mod questions;
#[path = "core/queue.rs"]
mod queue;
#[path = "core/sessions.rs"]
mod sessions;
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
  "capabilities": {{
    "usage": {{"version": 1}},
    "image_input": {{"version": 1}},
    "thinking": {{"version": 1}}
  }},
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
    test_core_with_deadlines(name, ProviderDeadlines::default())
}

fn test_core_with_deadlines(
    name: &str,
    deadlines: ProviderDeadlines,
) -> (tempfile::TempDir, MisyCore, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join(format!("{name}.txt"));
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    let core = MisyCore::discover_with_deadlines(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
        deadlines,
    )
    .expect("core discovery");
    (temporary, core, target)
}

fn test_core_with_questions(name: &str) -> (tempfile::TempDir, MisyCore, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join(format!("{name}.txt"));
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    let core = MisyCore::discover_with_options(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
        CoreOptions {
            client_capabilities: ClientCapabilities {
                question_request: Some(1),
            },
        },
    )
    .expect("core discovery");
    (temporary, core, target)
}

fn test_core_in_workspace(name: &str) -> (tempfile::TempDir, MisyCore, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let workspace = temporary.path().join("workspace");
    fs::create_dir_all(workspace.join(".git")).expect("project marker");
    fs::create_dir_all(workspace.join("frontend")).expect("frontend directory");
    fs::create_dir_all(workspace.join("backend")).expect("backend directory");
    fs::create_dir_all(temporary.path().join("misy")).expect("Misy directory");
    fs::write(temporary.path().join("misy/AGENTS.md"), "global rules").expect("global rules");
    fs::write(workspace.join("AGENTS.md"), "root rules").expect("root rules");
    fs::write(workspace.join("frontend/AGENTS.md"), "frontend rules").expect("frontend rules");
    fs::write(workspace.join("backend/AGENTS.md"), "backend rules").expect("backend rules");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &workspace);
    let core = MisyCore::discover_in_workspace(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
        &workspace,
    )
    .expect("core discovery");
    let frontend = workspace.join(format!("frontend/{name}.txt"));
    let backend = workspace.join(format!("backend/{name}.txt"));
    (temporary, core, frontend, backend)
}

fn fixture_model() -> ModelRef {
    ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model"))
}

async fn receive_until(
    events: &mut UnboundedReceiver<CoreEvent>,
    submission: SubmissionId,
    predicate: impl Fn(&CoreEvent) -> bool,
) -> Vec<CoreEvent> {
    let mut received = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("core event deadline")
            .expect("core event stream closed");
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
}

async fn receive_event(events: &mut UnboundedReceiver<CoreEvent>) -> CoreEvent {
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .expect("core event deadline")
        .expect("core event stream closed")
}

async fn wait_for_file(path: &Path) {
    for _ in 0..300 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(path.exists(), "fixture did not create {}", path.display());
}

fn fixture_signal(target: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{suffix}", target.display()))
}
