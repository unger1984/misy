//! Deadline, reaping, isolation, and shutdown tests for provider processes.

use super::{host_for, write_fixture_manifest};
use misy_core::{ProviderCatalog, ProviderDeadlines, ProviderError, ProviderHost, ProviderId};
use serde_json::json;
use std::{fs, path::Path, time::Duration};
use tokio::runtime::Handle;

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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
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
async fn request_deadline_reaps_an_unresponsive_provider_and_allows_restart() {
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
    let deadlines = ProviderDeadlines {
        request: Duration::from_millis(20),
        ..ProviderDeadlines::default()
    };
    let host = ProviderHost::with_handle_and_deadlines(
        ProviderCatalog::discover(&bundled, &installed).expect("catalog"),
        Handle::current(),
        deadlines,
    );
    let provider = ProviderId::new("fixture");

    // Unlike the bounded-request test above, this goes through the plain request path: the host
    // must apply its own deadline instead of waiting on the hung fixture forever.
    assert!(matches!(
        host.request(&provider, "test.hang", json!({})).await,
        Err(ProviderError::Timeout { .. })
    ));
    // The restart spawns a fresh subprocess, which outlives the 20ms host deadline whenever the
    // machine is loaded. Only recovery matters here, not its speed, so this wait is bounded
    // explicitly instead of inheriting the deadline the hang above needs to stay short.
    assert_eq!(
        host.request_with_timeout(&provider, "models.list", json!({}), Duration::from_secs(30))
            .await
            .expect("provider restart after request deadline"),
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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));

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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));

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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));

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
        let host = ProviderHost::with_handle(catalog, runtime.handle().clone());
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
