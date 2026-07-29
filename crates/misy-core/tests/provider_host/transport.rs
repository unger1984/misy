//! JSON-RPC correlation, cancellation, and protocol-error tests.

use super::{host_for, write_fixture_manifest};
use misy_core::{ProviderCatalog, ProviderError, ProviderId};
use serde_json::json;
use std::{fs, path::Path, time::Duration};

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
    let host = host_for(catalog);
    let mut events = host.subscribe();
    let provider = ProviderId::new("fixture");

    assert_eq!(host.running_provider_count(), 0);
    let mut slow = host
        .start_chat(&provider, json!({ "delay": "slow" }))
        .await
        .expect("slow request");
    let mut fast = host
        .start_chat(&provider, json!({ "delay": "fast" }))
        .await
        .expect("fast request");

    let fast_id = fast.id().get();
    let slow_id = slow.id().get();
    assert_eq!(
        fast.next_event()
            .await
            .expect("fast early event")
            .expect("fast stream event")
            .params["request_id"],
        json!(fast_id)
    );
    assert_eq!(
        slow.next_event()
            .await
            .expect("slow early event")
            .expect("slow stream event")
            .params["request_id"],
        json!(slow_id)
    );

    assert_eq!(
        fast.wait_response().await.expect("fast response"),
        json!({ "reply": "fast" })
    );
    assert_eq!(
        slow.wait_response().await.expect("slow response"),
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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
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
async fn host_rejects_oversized_protocol_line_and_allows_restart() {
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

    // The fixture streams 100 MiB without a newline; the host must fail fast against its
    // protocol line cap instead of buffering the stream until it runs out of memory.
    match host
        .request(&provider, "test.oversized_line", json!({}))
        .await
    {
        Err(ProviderError::Protocol { message, .. }) => {
            assert!(
                message.contains("limit"),
                "error must name the protocol line limit: {message}"
            );
        }
        other => panic!("expected oversized-line protocol error, got {other:?}"),
    }
    assert_eq!(
        host.request(&provider, "models.list", json!({}))
            .await
            .expect("provider restart after oversized line"),
        json!({ "models": [] })
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
    let host = host_for(ProviderCatalog::discover(&bundled, &installed).expect("catalog"));
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
    let host = std::sync::Arc::new(host_for(
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
