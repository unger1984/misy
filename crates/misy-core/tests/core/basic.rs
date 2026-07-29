//! Additional headless-core integration coverage.

use super::*;

#[tokio::test]
async fn core_discovers_authenticates_lists_and_persists_the_selected_model() {
    let (temporary, core, _) = test_core("setup");
    let provider = ProviderId::new("fixture");

    assert_eq!(core.providers().await.len(), 1);
    assert_eq!(
        core.auth_status(&provider).await.expect("auth status")["authenticated"],
        false
    );
    assert_eq!(
        core.start_auth(&provider).await.expect("auth start")["url"],
        "https://example.test/auth"
    );
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("auth complete");

    assert_eq!(
        core.list_models(&provider).await.expect("models")[0].model,
        fixture_model()
    );
    core.select_model(fixture_model())
        .await
        .expect("select model");
    assert_eq!(core.selected_model().await, Some(fixture_model()));
    drop(core);

    let reopened = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        temporary.path().join("bundled"),
    )
    .expect("reopen core");
    assert_eq!(reopened.selected_model().await, Some(fixture_model()));
    reopened.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_persists_auth_credentials_without_exposing_them_to_callers() {
    let (temporary, core, _) = test_core("redacted-auth");
    let provider = ProviderId::new("fixture");

    let completed = core
        .complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("auth complete");

    assert!(completed.get("credentials").is_none());
    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("stored credentials")
            .contains("opaque")
    );
    let refreshed = core.refresh_auth(&provider).await.expect("auth refresh");
    assert!(refreshed.get("credentials").is_none());
    assert!(
        fs::read_to_string(temporary.path().join("misy/credentials.json"))
            .expect("refreshed credentials")
            .contains("refreshed-opaque")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn usage_routes_to_the_models_provider_and_rejects_raw_provider_payloads() {
    let (_temporary, core, _) = test_core("usage");
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
    assert_eq!(report.limits[0].amount.used, Some(42.0));
    core.shutdown().await.expect("shutdown");

    let (_temporary, core, _) = test_core("bad-usage");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    let error = core
        .usage(&fixture_model())
        .await
        .expect_err("raw usage payload must be rejected")
        .to_string();
    assert!(error.contains("unknown field `raw`"));
    assert!(!error.contains("must-not-escape"));
    core.shutdown().await.expect("shutdown");
}
