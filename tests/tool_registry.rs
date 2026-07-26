use misy::{ToolDefinition, ToolRegistry};
use serde_json::json;

#[test]
fn registry_exposes_the_four_builtin_tool_definitions() {
    let registry = ToolRegistry::new();
    let definitions = registry.definitions();
    let names: Vec<_> = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();

    assert_eq!(
        names,
        ["list_directory", "read_file", "run_command", "write_file"]
    );
}

#[test]
fn registry_rejects_arguments_that_do_not_match_the_json_schema() {
    let registry = ToolRegistry::new();

    let error = registry
        .validate_arguments("read_file", &json!({"path": 42}))
        .unwrap_err();

    assert!(error.to_string().contains("path"));
}

#[test]
fn registry_rejects_a_duplicate_registration() {
    let mut registry = ToolRegistry::new();

    let error = registry
        .register(ToolDefinition::new(
            "read_file",
            "duplicate",
            json!({"type": "object"}),
        ))
        .unwrap_err();

    assert!(error.to_string().contains("already registered"));
}
