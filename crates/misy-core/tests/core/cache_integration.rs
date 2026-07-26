//! Additional headless-core integration coverage.

use super::*;

#[tokio::test]
async fn credential_method_is_read_locally_without_starting_a_provider() {
    let (_temporary, core, _) = test_core("credential-method");
    let provider = ProviderId::new("fixture");

    assert_eq!(
        core.credential_method(&provider).await.expect("method"),
        None
    );
    assert_eq!(core.running_provider_count().await, 0);

    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.shutdown().await.expect("shutdown");

    assert_eq!(
        core.credential_method(&provider).await.expect("method"),
        Some("oauth".to_owned())
    );
}

#[tokio::test]
async fn available_models_skips_unconfigured_providers_and_isolates_provider_failures() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("models.txt"),
    );
    write_fixture_manifest(
        &bundled,
        "failed-provider",
        &fixture,
        &temporary.path().join("bad-models.txt"),
    );
    write_fixture_manifest(
        &bundled,
        "unconfigured-provider",
        &fixture,
        &temporary.path().join("unconfigured.txt"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");

    for provider in [
        ProviderId::new("fixture"),
        ProviderId::new("failed-provider"),
    ] {
        core.complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("authenticate provider");
    }
    let available = core.available_models().await.expect("available models");

    assert_eq!(available.models.len(), 2);
    assert_eq!(available.errors.len(), 1);
    assert_eq!(available.errors[0].provider.as_str(), "failed-provider");
    assert_eq!(
        available.errors[0].provider_display_name,
        "failed-provider fixture"
    );
    assert_eq!(core.running_provider_count().await, 2);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cached_available_models_returns_models_without_provider_processes() {
    let (temporary, core, _) = test_core("cached-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    let expected = core.list_models(&provider).await.expect("list models");
    core.shutdown().await.expect("shutdown first core");

    let cached_core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        temporary.path().join("bundled"),
    )
    .expect("reopen core");

    assert_eq!(
        cached_core
            .cached_available_models()
            .await
            .expect("cached available models")
            .models,
        expected
    );
    assert_eq!(cached_core.running_provider_count().await, 0);
    cached_core.shutdown().await.expect("shutdown cached core");
}

#[tokio::test]
async fn logout_removes_the_provider_model_cache() {
    let (_temporary, core, _) = test_core("logout-model-cache");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.list_models(&provider).await.expect("list models");
    assert_eq!(
        core.cached_available_models()
            .await
            .expect("cached models")
            .models
            .len(),
        2
    );

    core.logout(&provider).await.expect("logout");

    assert!(
        core.cached_available_models()
            .await
            .expect("cached models after logout")
            .models
            .is_empty()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn late_model_response_cannot_restore_cache_after_logout() {
    let (_temporary, core, _) = test_core("slow-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");

    let listing = {
        let core = core.clone();
        let provider = provider.clone();
        tokio::spawn(async move { core.list_models(&provider).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    core.logout(&provider)
        .await
        .expect("logout while listing models");
    listing
        .await
        .expect("model-listing thread")
        .expect("model listing");

    assert!(
        core.cached_available_models()
            .await
            .expect("cached models after logout")
            .models
            .is_empty()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pre_logout_model_response_cannot_populate_a_reauthenticated_catalog() {
    let (temporary, core, target) = test_core("async-gated-models");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "first"}),
    )
    .await
    .expect("first authentication");

    let listing = {
        let core = core.clone();
        let provider = provider.clone();
        tokio::spawn(async move { core.list_models(&provider).await })
    };
    let started = fixture_signal(&target, "started");
    wait_for_file(&started).await;
    core.logout(&provider)
        .await
        .expect("logout after request starts");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "second"}),
    )
    .await
    .expect("second authentication");
    fs::write(fixture_signal(&target, "release"), "release stale response")
        .expect("release stale response");
    listing
        .await
        .expect("model-listing thread")
        .expect("delayed model listing");

    assert!(
        core.cached_available_models()
            .await
            .expect("cached models after reauthentication")
            .models
            .is_empty()
    );
    core.shutdown().await.expect("shutdown");
    drop(temporary);
}
