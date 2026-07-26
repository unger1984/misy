//! Subscription and state-snapshot coverage for the headless core.

use super::{fixture_model, receive_until, test_core};
use misy_core::{CoreEvent, Message, ProviderAuthState, ProviderId};
use serde_json::json;

#[tokio::test]
async fn subscription_replays_the_discovered_provider_snapshot() {
    let (_temporary, core, _) = test_core("provider-snapshot");
    let mut events = core.subscribe_lossless();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("provider snapshot deadline")
            .expect("provider snapshot"),
        CoreEvent::ProviderDiscovered { provider } if provider.as_str() == "fixture"
    ));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn state_snapshot_tracks_model_authentication_and_fifo_cancellation() {
    let (_temporary, core, _) = test_core("core-state-snapshot");
    let provider = ProviderId::new("fixture");

    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider.clone(),
            authenticated: false,
            credential_method: None,
        }]
    );

    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider.clone(),
            authenticated: true,
            credential_method: Some("oauth".to_owned()),
        }]
    );

    core.select_model(fixture_model())
        .await
        .expect("select model");
    assert_eq!(core.snapshot().selected_model, Some(fixture_model()));

    let mut events = core.subscribe_lossless();
    let active = core
        .submit(Message::user("block-session"))
        .await
        .expect("active submission");
    receive_until(&mut events, active, |event| {
        matches!(event, CoreEvent::SubmissionStarted { submission, .. } if *submission == active)
    })
    .await;
    let queued = core
        .submit(Message::user("cancel-me"))
        .await
        .expect("queued submission");
    let snapshot = core.snapshot();
    assert_eq!(snapshot.active_submission, Some(active));
    assert_eq!(snapshot.queued_submissions, vec![queued]);

    core.cancel(queued).await.expect("cancel queued submission");
    let snapshot = core.snapshot();
    assert_eq!(snapshot.active_submission, Some(active));
    assert!(snapshot.queued_submissions.is_empty());

    receive_until(
        &mut events,
        active,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == active),
    )
    .await;
    receive_until(
        &mut events,
        queued,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == queued),
    )
    .await;
    assert!(core.snapshot().active_submission.is_none());
    assert!(core.snapshot().queued_submissions.is_empty());

    core.logout(&provider).await.expect("logout");
    assert_eq!(
        core.snapshot().providers,
        vec![ProviderAuthState {
            id: provider,
            authenticated: false,
            credential_method: None,
        }]
    );
    core.shutdown().await.expect("shutdown");
}
