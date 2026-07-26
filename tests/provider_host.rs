use misy::{
    PROVIDER_PROTOCOL_VERSION, ProviderCatalog, ProviderDiscoveryError, ProviderError,
    ProviderHost, ProviderId,
};
use serde_json::json;
use std::{fs, path::Path, time::Duration};

fn write_manifest(root: &Path, id: &str, protocol_version: u32) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "version": "1.2.3",
  "kind": "provider",
  "protocol_version": {protocol_version},
  "description": "A fixture provider",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "fixture-provider",
  "args": []
}}"#
        ),
    )
    .expect("manifest");
}

#[test]
fn discovery_reads_self_contained_provider_packages() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "bundled-provider", PROVIDER_PROTOCOL_VERSION);
    write_manifest(&installed, "installed-provider", PROVIDER_PROTOCOL_VERSION);

    let catalog = ProviderCatalog::discover(&bundled, &installed).expect("valid packages");

    assert_eq!(catalog.len(), 2);
    assert!(catalog.get("bundled-provider").is_some());
    assert!(catalog.get("installed-provider").is_some());
}

#[test]
fn discovery_rejects_duplicate_and_incompatible_packages() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "duplicate", PROVIDER_PROTOCOL_VERSION);
    write_manifest(&installed, "duplicate", PROVIDER_PROTOCOL_VERSION);

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::DuplicateProvider(_))
    ));

    fs::remove_dir_all(&installed).expect("remove duplicate");
    write_manifest(&installed, "incompatible", PROVIDER_PROTOCOL_VERSION + 1);
    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::UnsupportedProtocol { .. })
    ));
}

#[test]
fn discovery_rejects_manifest_without_required_metadata() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "metadata", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("metadata/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "\"description\": \"A fixture provider\"",
            "\"description\": \"\"",
        ),
    )
    .expect("empty metadata");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifestValue { .. })
    ));
}

fn write_fixture_manifest(root: &Path, id: &str, fixture: &Path, log_file: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    let fixture = fixture.to_string_lossy();
    let log_file = log_file.to_string_lossy();
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 1,
  "description": "Language-neutral fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{fixture}",
  "args": ["{log_file}"]
}}"#
        ),
    )
    .expect("manifest");
}

#[test]
fn host_lazily_correlates_concurrent_requests_and_forwards_notifications() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    let log_file = temporary.path().join("provider.log");
    write_fixture_manifest(
        &bundled,
        "fixture",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/provider_fixture.sh")
            .as_path(),
        &log_file,
    );
    let catalog = ProviderCatalog::discover(&bundled, &installed).expect("catalog");
    let host = ProviderHost::new(catalog);
    let events = host.subscribe();
    let provider = ProviderId::new("fixture");

    assert_eq!(host.running_provider_count(), 0);
    let slow = host
        .request_async(&provider, "chat.start", json!({ "delay": "slow" }))
        .expect("slow request");
    let fast = host
        .request_async(&provider, "chat.start", json!({ "delay": "fast" }))
        .expect("fast request");

    assert_eq!(
        fast.wait().expect("fast response"),
        json!({ "reply": "fast" })
    );
    assert_eq!(
        slow.wait().expect("slow response"),
        json!({ "reply": "slow" })
    );
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(1))
            .expect("stream event")
            .method,
        "text_delta"
    );
    assert_eq!(host.running_provider_count(), 1);

    host.shutdown().expect("clean shutdown");
    assert_eq!(host.running_provider_count(), 0);
    let starts = fs::read_to_string(log_file).expect("fixture log");
    assert_eq!(starts.lines().filter(|line| *line == "started").count(), 1);
}

#[test]
fn host_forwards_cancellation_to_the_pending_provider_request() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    let log_file = temporary.path().join("provider.log");
    write_fixture_manifest(
        &bundled,
        "fixture",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/provider_fixture.sh")
            .as_path(),
        &log_file,
    );
    let host = ProviderHost::new(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
    let provider = ProviderId::new("fixture");
    let pending = host
        .request_async(&provider, "chat.start", json!({ "delay": "slow" }))
        .expect("pending request");

    host.cancel_request(&provider, pending.id())
        .expect("forward cancellation");
    assert!(matches!(pending.wait(), Err(ProviderError::Cancelled(_))));

    for _ in 0..20 {
        if fs::read_to_string(&log_file)
            .map(|contents| contents.contains("cancelled"))
            .unwrap_or(false)
        {
            host.shutdown().expect("shutdown");
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("fixture did not receive JSON-RPC cancellation");
}

#[test]
fn host_rejects_malformed_and_eof_provider_output_without_poisoning_restarts() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    let log_file = temporary.path().join("provider.log");
    write_fixture_manifest(
        &bundled,
        "fixture",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/provider_fixture.sh")
            .as_path(),
        &log_file,
    );
    let host = ProviderHost::new(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
    let provider = ProviderId::new("fixture");

    assert!(matches!(
        host.request(&provider, "test.malformed", json!({})),
        Err(ProviderError::Protocol { .. })
    ));
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .expect("provider restart after malformed output"),
        json!({ "models": [] })
    );
    assert!(matches!(
        host.request(&provider, "test.exit", json!({})),
        Err(ProviderError::Transport { .. })
    ));
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .expect("provider restart after EOF"),
        json!({ "models": [] })
    );
    host.shutdown().expect("shutdown");
}

#[test]
fn one_failed_provider_does_not_interrupt_another_provider() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "broken",
        &fixture,
        &temporary.path().join("broken.log"),
    );
    write_fixture_manifest(
        &installed,
        "healthy",
        &fixture,
        &temporary.path().join("healthy.log"),
    );
    let host = ProviderHost::new(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));

    assert!(matches!(
        host.request(&ProviderId::new("broken"), "test.malformed", json!({})),
        Err(ProviderError::Protocol { .. })
    ));
    assert_eq!(
        host.request(&ProviderId::new("healthy"), "models.list", json!({}))
            .expect("unrelated provider remains available"),
        json!({ "models": [] })
    );
    host.shutdown().expect("shutdown");
}
