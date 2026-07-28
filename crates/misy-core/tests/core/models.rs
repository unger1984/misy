//! Model-catalog behavior exercised through the public core contract.

use super::*;

#[tokio::test]
async fn core_selects_the_provider_declared_default_model() {
    let (_temporary, core, _) = test_core("explicit-default");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("auth complete");

    let selected = core
        .select_default_model(&provider)
        .await
        .expect("select provider default");

    assert_eq!(
        selected,
        ModelRef::new(provider, ModelId::new("fixture-model-b"))
    );
    assert_eq!(
        core.cached_available_models()
            .await
            .expect("cached models")
            .models
            .len(),
        2
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn core_refuses_to_clobber_an_unreadable_config_when_selecting_a_model() {
    let (temporary, core, _) = test_core("config-read-modify-write");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("auth complete");

    // Persisting a selection must read the existing file first: the previous
    // build-a-fresh-`Config` write would erase every other field once `Config` grows.
    // `Config` silently ignores unknown TOML keys today, so a corrupt file is the only
    // on-disk proof that the write path really loaded what it is about to replace.
    let config_file = temporary.path().join("misy").join("config.toml");
    fs::create_dir_all(config_file.parent().expect("config file has a parent"))
        .expect("create data directory");
    fs::write(&config_file, "version = \"not-a-number\"\n").expect("corrupt the config");

    let error = core
        .select_model(fixture_model())
        .await
        .expect_err("an unreadable config must fail the selection");

    assert!(error.to_string().contains("invalid config file"));
    assert_eq!(
        fs::read_to_string(&config_file).expect("config file must survive"),
        "version = \"not-a-number\"\n"
    );
    core.shutdown().await.expect("shutdown");
}
