//! Domain-contract integration tests.

use misy_core::{
    InputModality, Message, ModelId, ModelInfo, ModelProfile, ModelRef, ProviderId, ToolCall,
    ToolDefinition, ToolResult,
};
use serde_json::json;
use std::str::FromStr;

#[test]
fn normalized_domain_types_round_trip_through_json() {
    let model = ModelRef::new(ProviderId::new("codex"), ModelId::new("gpt-5"));
    let info = ModelInfo::new(model.clone(), "GPT-5", 128_000);
    let message = Message::user("Hello");
    let definition = ToolDefinition::new(
        "read_file",
        "Read a UTF-8 file",
        json!({"type": "object", "required": ["path"]}),
    );
    let call = ToolCall::new("call-1", "read_file", json!({"path": "README.md"}));
    let result = ToolResult::success("call-1", "contents");

    assert_eq!(model.provider.as_str(), "codex");
    assert_eq!(model.model.as_str(), "gpt-5");
    assert_eq!(info.context_window, 128_000);
    assert_eq!(message.content, "Hello");
    assert_eq!(definition.name, "read_file");
    assert_eq!(call.arguments["path"], "README.md");
    assert!(!result.is_error);
    assert_eq!(
        serde_json::from_str::<ModelRef>(
            &serde_json::to_string(&model).expect("serialize model reference"),
        )
        .expect("deserialize model reference"),
        model
    );
}

#[test]
fn legacy_model_json_defaults_to_text_only() {
    let model: ModelInfo = serde_json::from_value(json!({
        "model": {"provider": "legacy", "model": "text-model"},
        "display_name": "Legacy text model",
        "context_window": 8_192
    }))
    .expect("deserialize legacy model info");

    assert_eq!(model.input_modalities, vec![InputModality::Text]);
}

#[test]
fn selector_preserves_model_colons_without_a_catalog() {
    let profile = ModelProfile::from_str("anymodel/cx/gpt%3Ahigh:low")
        .expect("selector with an escaped model colon");

    assert_eq!(profile.model.provider.as_str(), "anymodel");
    assert_eq!(profile.model.model.as_str(), "cx/gpt:high");
    assert_eq!(profile.thinking.as_deref(), Some("low"));
    assert_eq!(profile.selector(), "anymodel/cx/gpt%3Ahigh:low");
}

#[test]
fn selector_rejects_empty_thinking_and_unrecognized_escapes() {
    assert!(ModelProfile::from_str("openai/gpt:").is_err());
    assert!(ModelProfile::from_str("openai/gpt%2F4").is_err());
    assert!(ModelProfile::from_str("openai/gpt:High").is_err());
}
