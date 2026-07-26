//! Authentication-method contract tests.

use super::{fixture_model, receive_until, test_core};
use misy::{CoreError, CoreEvent, Message, ProviderId};
use serde_json::json;

#[test]
fn auth_start_validates_and_forwards_the_declared_method() {
    let (_temporary, core, _) = test_core("auth-method");
    let provider = ProviderId::new("fixture");

    assert_eq!(
        core.start_auth_with_method(&provider, "oauth")
            .expect("declared auth method")["url"],
        "https://example.test/auth"
    );
    assert!(matches!(
        core.start_auth_with_method(&provider, "api_key"),
        Err(CoreError::UnsupportedAuthMethod { method, .. }) if method == "api_key"
    ));
    core.shutdown().expect("shutdown");
}

#[test]
fn expiring_credentials_are_refreshed_and_persisted_before_chat() {
    let (_temporary, core, _) = test_core("refresh-before-chat");
    let provider = ProviderId::new("fixture");
    core.complete_auth(&provider, json!({"id": "expired"}), json!({}))
        .expect("store expiring credentials");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("refresh-before-chat"))
        .expect("submit chat");
    let received = receive_until(
        &events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    );
    assert!(
        received.iter().any(
            |event| matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "refreshed")
        )
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn auth_complete_keeps_session_separate_from_completion() {
    let (_temporary, core, _) = test_core("separate-auth-params");
    let provider = ProviderId::new("fixture");

    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"id": "not-the-session"}),
    )
    .expect("session is forwarded separately");
    assert!(
        core.complete_auth(
            &provider,
            json!({"id": "not-the-session"}),
            json!({"id": "fixture-session"}),
        )
        .is_err()
    );
    core.shutdown().expect("shutdown");
}
