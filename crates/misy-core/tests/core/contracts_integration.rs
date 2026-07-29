//! Additional headless-core integration coverage.

use super::*;

#[tokio::test]
async fn core_passes_stored_credentials_to_streaming_chat_requests() {
    let (_temporary, core, _) = test_core("chat-credentials");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("credential-chat"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(received.iter().any(|event| {
        matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "authenticated")
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_sanitizes_all_public_auth_responses() {
    let (_temporary, core, _) = test_core("sanitized-auth");
    let provider = ProviderId::new("fixture");

    assert!(
        core.auth_status(&provider)
            .await
            .expect("status")
            .get("credentials")
            .is_none()
    );
    assert!(
        core.start_auth(&provider)
            .await
            .expect("start")
            .get("credentials")
            .is_none()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_recursively_sanitizes_successful_auth_results_after_persisting_top_level_credentials()
{
    let (temporary, core, _) = test_core("nested-auth");
    let provider = ProviderId::new("fixture");

    for response in [
        core.auth_status(&provider).await.expect("status"),
        core.start_auth(&provider).await.expect("start"),
        core.complete_auth(
            &provider,
            json!({"id":"fixture-session"}),
            json!({"code":"nested"}),
        )
        .await
        .expect("complete"),
        core.refresh_auth(&provider).await.expect("refresh"),
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
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_sanitizes_credentials_from_remote_auth_errors() {
    let (_temporary, core, _) = test_core("remote-auth-error");
    let provider = ProviderId::new("fixture");
    let error = core
        .complete_auth(&provider, json!({"id":"remote-error"}), json!({}))
        .await
        .expect_err("remote auth failure");
    let misy_core::CoreError::Provider(misy_core::ProviderError::Remote { data, .. }) = error
    else {
        panic!("expected remote provider error");
    };
    assert!(
        !serde_json::to_string(&data)
            .expect("error data")
            .contains("secret")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_redacts_secret_shaped_fragments_from_remote_error_messages() {
    let (_temporary, core, _) = test_core("redacted-remote-message");
    let provider = ProviderId::new("fixture");
    let error = core
        .complete_auth(&provider, json!({"id":"bearer-error"}), json!({}))
        .await
        .expect_err("remote auth failure");
    let text = error.to_string();
    assert!(text.contains("[redacted]"), "error text: {text}");
    assert!(!text.contains("sk-fixture-leaked-token-0123456789"));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_redacts_stream_failure_messages_before_they_reach_events() {
    let (_temporary, core, _) = test_core("redacted-stream-failure");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("leak-bearer-error"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Failed { submission: id, .. } if *id == submission),
    )
    .await;

    for event in &received {
        assert!(!format!("{event:?}").contains("sk-fixture-leaked-chat-token-0123456789"));
    }
    let message = received
        .iter()
        .find_map(|event| match event {
            CoreEvent::Failed { message, .. } => Some(message),
            _ => None,
        })
        .expect("failed event");
    assert!(message.contains("[redacted]"));
    assert!(
        !format!("{:?}", core.history().await).contains("sk-fixture-leaked-chat-token-0123456789")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_truncates_oversized_stream_failure_messages() {
    let (_temporary, core, _) = test_core("truncated-stream-failure");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("huge-error"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Failed { submission: id, .. } if *id == submission),
    )
    .await;

    let message = received
        .iter()
        .find_map(|event| match event {
            CoreEvent::Failed { message, .. } => Some(message),
            _ => None,
        })
        .expect("failed event");
    assert!(message.contains("truncated: showing the first"));
    assert!(message.len() < 1024 + 80);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subscription_snapshot_includes_more_than_the_default_event_buffer() {
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
    let mut events = core.subscribe_lossless();
    let mut providers = std::collections::BTreeSet::new();
    for _ in 0..129 {
        let CoreEvent::ProviderDiscovered { provider } = receive_event(&mut events).await else {
            panic!("only provider snapshot events are expected");
        };
        providers.insert(provider.as_str().to_owned());
    }
    assert_eq!(providers.len(), 129);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn chat_result_credentials_are_persisted_without_reaching_clients() {
    let (temporary, core, _) = test_core("rotate-chat-credentials");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("rotate-credentials-chat"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;

    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("stored credentials")
            .contains("chat-rotated")
    );
    for event in &received {
        assert!(!format!("{event:?}").contains("chat-rotated"));
    }
    assert!(!format!("{:?}", core.history().await).contains("chat-rotated"));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn usage_result_credentials_are_persisted_without_reaching_clients() {
    let (temporary, core, _) = test_core("usage-rotate");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");

    let report = core.usage(&fixture_model()).await.expect("usage report");

    assert_eq!(report.limits[0].id, "five-hour");
    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("stored credentials")
            .contains("usage-rotated")
    );
    assert!(!format!("{report:?}").contains("usage-rotated"));
    core.shutdown().await.expect("shutdown");
}
