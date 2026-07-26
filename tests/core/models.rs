//! Model-catalog behavior exercised through the public core contract.

use super::*;

#[test]
fn core_selects_the_provider_declared_default_model() {
    let (_temporary, core, _) = test_core("explicit-default");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("auth complete");

    let selected = core
        .select_default_model(&provider)
        .expect("select provider default");

    assert_eq!(
        selected,
        ModelRef::new(provider, ModelId::new("fixture-model-b"))
    );
    assert_eq!(
        core.cached_available_models()
            .expect("cached models")
            .models
            .len(),
        2
    );
    core.shutdown().expect("shutdown");
}
