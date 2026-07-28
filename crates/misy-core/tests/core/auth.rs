//! Authentication-method contract tests.

use super::{fixture_model, receive_event, receive_until, test_core};
use misy_core::{CoreError, CoreEvent, Message, ProviderAuthState, ProviderError, ProviderId};
use serde_json::json;

#[tokio::test]
async fn auth_start_validates_and_forwards_the_declared_method() {
    let (_temporary, core, _) = test_core("auth-method");
    let provider = ProviderId::new("fixture");

    assert_eq!(
        core.start_auth_with_method(&provider, "oauth")
            .await
            .expect("declared auth method")["url"],
        "https://example.test/auth"
    );
    assert!(matches!(
        core.start_auth_with_method(&provider, "api_key").await,
        Err(CoreError::UnsupportedAuthMethod { method, .. }) if method == "api_key"
    ));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn no_auth_flow_updates_the_snapshot_and_emits_one_authentication_event() {
    let (temporary, core, _) = test_core("auth-none");
    let provider = ProviderId::new("fixture");
    let mut events = core.subscribe_lossless();

    assert_eq!(
        core.start_auth(&provider).await.expect("start")["kind"],
        "none"
    );
    assert!(matches!(
        receive_event(&mut events).await,
        CoreEvent::ProviderDiscovered { provider: id } if id == provider
    ));
    assert!(matches!(
        receive_event(&mut events).await,
        CoreEvent::AuthenticationChanged {
            provider: id,
            authenticated: true,
        } if id == provider
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv())
            .await
            .is_err()
    );
    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider,
            authenticated: true,
            credential_method: Some("oauth".to_owned()),
        }]
    );
    assert!(!temporary.path().join("misy/credentials.json").exists());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refresh_updates_the_authentication_snapshot_and_emits_one_event() {
    let (_temporary, core, _) = test_core("refresh-auth-snapshot");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    let mut events = core.subscribe_lossless();

    let response = core.refresh_auth(&provider).await.expect("refresh");
    assert!(response.get("credentials").is_none());
    assert!(matches!(
        receive_event(&mut events).await,
        CoreEvent::ProviderDiscovered { provider: id } if id == provider
    ));
    assert!(matches!(
        receive_event(&mut events).await,
        CoreEvent::AuthenticationChanged {
            provider: id,
            authenticated: true,
        } if id == provider
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv())
            .await
            .is_err()
    );
    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider,
            authenticated: true,
            credential_method: Some("oauth".to_owned()),
        }]
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn expiring_credentials_are_refreshed_and_persisted_before_chat() {
    let (_temporary, core, _) = test_core("refresh-before-chat");
    let provider = ProviderId::new("fixture");
    core.complete_auth(&provider, json!({"id": "expired"}), json!({}))
        .await
        .expect("store expiring credentials");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("refresh-before-chat"))
        .await
        .expect("submit chat");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(
        received.iter().any(
            |event| matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "refreshed")
        )
    );
    assert_eq!(
        received
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    CoreEvent::AuthenticationChanged {
                        provider,
                        authenticated: true,
                    } if provider.as_str() == "fixture"
                )
            })
            .count(),
        1
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn auth_complete_keeps_session_separate_from_completion() {
    let (_temporary, core, _) = test_core("separate-auth-params");
    let provider = ProviderId::new("fixture");

    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"id": "not-the-session"}),
    )
    .await
    .expect("session is forwarded separately");
    assert!(
        core.complete_auth(
            &provider,
            json!({"id": "not-the-session"}),
            json!({"id": "fixture-session"}),
        )
        .await
        .is_err()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn complete_auth_without_credentials_fails_and_changes_no_state() {
    let (temporary, core, _) = test_core("complete-no-credentials");
    let provider = ProviderId::new("fixture");
    let mut events = core.subscribe_lossless();

    let error = core
        .complete_auth(&provider, json!({"id": "no-credentials"}), json!({}))
        .await
        .expect_err("credential-less completion violates protocol v2");
    assert!(matches!(
        error,
        CoreError::Provider(ProviderError::Protocol { .. })
    ));
    assert!(matches!(
        receive_event(&mut events).await,
        CoreEvent::ProviderDiscovered { provider: id } if id == provider
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv())
            .await
            .is_err(),
        "a rejected completion must not emit AuthenticationChanged"
    );
    assert!(
        !core
            .has_credentials(&provider)
            .await
            .expect("has credentials")
    );
    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider,
            authenticated: false,
            credential_method: None,
        }]
    );
    assert!(
        core.cached_available_models()
            .await
            .expect("cached models")
            .models
            .is_empty(),
        "a rejected completion must not touch the model cache"
    );
    assert!(!temporary.path().join("misy/credentials.json").exists());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refresh_without_credentials_fails_and_keeps_the_recorded_state() {
    let (_temporary, core, _) = test_core("refresh-no-credentials");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");

    let error = core
        .refresh_auth(&provider)
        .await
        .expect_err("credential-less refresh violates protocol v2");
    assert!(matches!(
        error,
        CoreError::Provider(ProviderError::Protocol { .. })
    ));
    assert!(
        core.has_credentials(&provider)
            .await
            .expect("has credentials")
    );
    assert_eq!(
        core.credential_method(&provider).await.expect("method"),
        Some("oauth".to_owned())
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn none_flow_refresh_without_credentials_stays_authenticated() {
    let (temporary, core, _) = test_core("auth-none");
    let provider = ProviderId::new("fixture");
    core.start_auth(&provider).await.expect("none-flow start");

    core.refresh_auth(&provider)
        .await
        .expect("a none-flow provider legitimately holds no credentials");
    assert!(
        core.has_credentials(&provider)
            .await
            .expect("has credentials")
    );
    assert_eq!(
        core.credential_method(&provider).await.expect("method"),
        Some("oauth".to_owned())
    );
    assert!(!temporary.path().join("misy/credentials.json").exists());
    core.shutdown().await.expect("shutdown");
}
