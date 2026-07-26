//! Domain-contract integration tests.

use misy::{
    Message, ModelId, ModelInfo, ModelRef, ProviderId, ToolCall, ToolDefinition, ToolResult,
};
use serde_json::json;

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
