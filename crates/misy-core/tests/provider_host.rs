//! Provider-host integration tests.

use misy_core::{
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
  "display_name": "{id} fixture",
  "version": "1.2.3",
  "kind": "provider",
  "protocol_version": {protocol_version},
  "description": "A fixture provider",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "fixture-provider",
  "args": [],
  "auth_methods": [{{"id": "oauth", "display_name": "Fixture OAuth"}}]
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
    assert_eq!(
        catalog
            .get("bundled-provider")
            .expect("bundled provider")
            .manifest()
            .capabilities,
        Default::default()
    );
    assert!(catalog.get("installed-provider").is_some());
}

#[test]
fn discovery_reads_versioned_optional_capabilities() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "usage-provider", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("usage-provider/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"description\": \"A fixture provider\",\n",
            "  \"description\": \"A fixture provider\",\n  \"capabilities\": {\"usage\": {\"version\": 1}},\n",
        ),
    )
    .expect("usage capability");

    let catalog = ProviderCatalog::discover(&bundled, &installed).expect("valid capability");
    let package = catalog.get("usage-provider").expect("usage provider");

    assert!(package.manifest().supports_capability("usage", 1));
    assert!(!package.manifest().supports_capability("usage", 2));
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

#[test]
fn discovery_requires_display_name_and_authentication_methods() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "missing-display-name", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-display-name/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"display_name\": \"missing-display-name fixture\",\n",
            "",
        ),
    )
    .expect("missing display name");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifest { .. })
    ));

    fs::remove_dir_all(bundled.join("missing-display-name")).expect("remove invalid manifest");
    write_manifest(&bundled, "missing-auth-methods", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-auth-methods/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"auth_methods\": [{\"id\": \"oauth\", \"display_name\": \"Fixture OAuth\"}]\n",
            "  \"auth_methods\": []\n",
        ),
    )
    .expect("empty auth methods");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifestValue { .. })
    ));
}

#[test]
fn discovery_rejects_manifest_without_args() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let installed = temporary.path().join("installed");
    write_manifest(&bundled, "missing-args", PROVIDER_PROTOCOL_VERSION);
    let manifest = bundled.join("missing-args/misy-plugin.json");
    let contents = fs::read_to_string(&manifest).expect("manifest");
    fs::write(
        &manifest,
        contents.replace(
            "  \"command\": \"fixture-provider\",\n  \"args\": [],\n",
            "  \"command\": \"fixture-provider\",\n",
        ),
    )
    .expect("missing args");

    assert!(matches!(
        ProviderCatalog::discover(&bundled, &installed),
        Err(ProviderDiscoveryError::InvalidManifest { .. })
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
  "display_name": "{id} fixture",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 2,
  "description": "Language-neutral fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{fixture}",
  "args": ["{log_file}"],
  "auth_methods": [{{"id": "oauth", "display_name": "Fixture OAuth"}}]
}}"#
        ),
    )
    .expect("manifest");
}

#[tokio::test]
async fn host_lazily_correlates_concurrent_requests_and_forwards_notifications() {
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
    let mut events = host.subscribe();
    let provider = ProviderId::new("fixture");

    assert_eq!(host.running_provider_count(), 0);
    let slow = host
        .request_async(&provider, "chat.start", json!({ "delay": "slow" }))
        .await
        .expect("slow request");
    let fast = host
        .request_async(&provider, "chat.start", json!({ "delay": "fast" }))
        .await
        .expect("fast request");

    assert_eq!(
        fast.wait().await.expect("fast response"),
        json!({ "reply": "fast" })
    );
    assert_eq!(
        slow.wait().await.expect("slow response"),
        json!({ "reply": "slow" })
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("stream event deadline")
            .expect("stream event")
            .method,
        "text_delta"
    );
    assert_eq!(host.running_provider_count(), 1);

    host.shutdown().await.expect("clean shutdown");
    assert_eq!(host.running_provider_count(), 0);
    let starts = fs::read_to_string(log_file).expect("fixture log");
    assert_eq!(starts.lines().filter(|line| *line == "started").count(), 1);
}

#[tokio::test]
async fn host_forwards_cancellation_to_the_pending_provider_request() {
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
        .await
        .expect("pending request");

    host.cancel_request(&provider, pending.id())
        .await
        .expect("forward cancellation");
    assert!(matches!(
        pending.wait().await,
        Err(ProviderError::Cancelled(_))
    ));

    for _ in 0..20 {
        if fs::read_to_string(&log_file)
            .map(|contents| contents.contains("cancelled"))
            .unwrap_or(false)
        {
            host.shutdown().await.expect("shutdown");
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture did not receive JSON-RPC cancellation");
}

#[tokio::test]
async fn host_rejects_malformed_and_eof_provider_output_without_poisoning_restarts() {
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
        host.request(&provider, "test.malformed", json!({})).await,
        Err(ProviderError::Protocol { .. })
    ));
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .await
            .expect("provider restart after malformed output"),
        json!({ "models": [] })
    );
    assert!(matches!(
        host.request(&provider, "test.exit", json!({})).await,
        Err(ProviderError::Transport { .. })
    ));
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .await
            .expect("provider restart after EOF"),
        json!({ "models": [] })
    );
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn bounded_request_reaps_an_unresponsive_provider_and_allows_restart() {
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
        host.request_with_timeout(&provider, "test.hang", json!({}), Duration::from_millis(20))
            .await,
        Err(ProviderError::Timeout { .. })
    ));
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .await
            .expect("provider restart after timeout"),
        json!({ "models": [] })
    );
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn one_failed_provider_does_not_interrupt_another_provider() {
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
        host.request(&ProviderId::new("broken"), "test.malformed", json!({}))
            .await,
        Err(ProviderError::Protocol { .. })
    ));
    assert_eq!(
        host.request(&ProviderId::new("healthy"), "models.list", json!({}))
            .await
            .expect("unrelated provider remains available"),
        json!({ "models": [] })
    );
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn malformed_provider_is_terminated_and_reaped() {
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

    assert!(matches!(
        host.request(
            &ProviderId::new("fixture"),
            "test.malformed_stay_alive",
            json!({}),
        )
        .await,
        Err(ProviderError::Protocol { .. })
    ));
    let mut pid = None;
    for _ in 0..20 {
        pid = fs::read_to_string(&log_file).ok().and_then(|contents| {
            contents
                .lines()
                .find_map(|line| line.strip_prefix("pid:")?.parse::<u32>().ok())
        });
        if pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = pid.expect("fixture pid");
    let reaped = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let output = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .expect("check process liveness");
            if !output.status.success() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(reaped.is_ok(), "malformed provider process must be reaped");
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn malformed_provider_termination_reaps_long_lived_descendants() {
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

    assert!(matches!(
        host.request(
            &ProviderId::new("fixture"),
            "test.malformed_descendant",
            json!({}),
        )
        .await,
        Err(ProviderError::Protocol { .. })
    ));
    let mut descendant = None;
    for _ in 0..20 {
        descendant = fs::read_to_string(&log_file).ok().and_then(|contents| {
            contents
                .lines()
                .find_map(|line| line.strip_prefix("descendant:")?.parse::<u32>().ok())
        });
        if descendant.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let descendant = descendant.expect("fixture descendant pid");
    let output = std::process::Command::new("kill")
        .args(["-0", &descendant.to_string()])
        .output()
        .expect("check descendant liveness");
    assert!(
        !output.status.success(),
        "provider descendants must be terminated with their process group"
    );
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unread_subscriber_does_not_block_flooded_provider() {
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
    let _unread = host.subscribe();

    let pending = host
        .request_async(&ProviderId::new("fixture"), "test.flood", json!({}))
        .await
        .expect("flood request");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), pending.wait())
            .await
            .expect("provider processing must not block")
            .expect("flood response"),
        json!({ "flooded": true })
    );
    host.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_requests_complete_when_provider_exits() {
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
    let host = std::sync::Arc::new(ProviderHost::new(
        ProviderCatalog::discover(&bundled, &installed).expect("catalog"),
    ));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let host = std::sync::Arc::clone(&host);
        workers.spawn(async move {
            host.request(&ProviderId::new("fixture"), "test.exit", json!({}))
                .await
        });
    }
    while let Some(worker) = workers.join_next().await {
        assert!(matches!(
            worker.expect("worker"),
            Err(ProviderError::Transport { .. }) | Err(ProviderError::Shutdown)
        ));
    }
    host.shutdown().await.expect("shutdown");
}

#[test]
fn standalone_host_drop_after_caller_runtime_reaps_leader_and_descendant() {
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
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("caller runtime");
        let host = ProviderHost::new(catalog);
        let result = runtime.block_on(host.request(
            &ProviderId::new("fixture"),
            "test.healthy_descendant",
            json!({}),
        ));
        assert_eq!(
            result.expect("healthy response"),
            json!({ "healthy": true })
        );
        drop(runtime);
        drop(host);
    })
    .join()
    .expect("caller thread");

    let (leader, descendant) = (0..100)
        .find_map(|_| {
            let contents = fs::read_to_string(&log_file).ok()?;
            let leader = contents
                .lines()
                .find_map(|line| line.strip_prefix("pid:")?.parse::<u32>().ok());
            let descendant = contents
                .lines()
                .find_map(|line| line.strip_prefix("descendant:")?.parse::<u32>().ok());
            match (leader, descendant) {
                (Some(leader), Some(descendant)) => Some((leader, descendant)),
                _ => {
                    std::thread::sleep(Duration::from_millis(10));
                    None
                }
            }
        })
        .expect("fixture process ids");
    for _ in 0..100 {
        let leader_alive = std::process::Command::new("kill")
            .args(["-0", &leader.to_string()])
            .output()
            .expect("check leader liveness")
            .status
            .success();
        let descendant_alive = std::process::Command::new("kill")
            .args(["-0", &descendant.to_string()])
            .output()
            .expect("check descendant liveness")
            .status
            .success();
        if !leader_alive && !descendant_alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("standalone host drop must reap provider leader and descendants");
}
