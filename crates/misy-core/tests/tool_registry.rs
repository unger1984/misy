//! Tool-registry integration tests.

use misy_core::{ToolDefinition, ToolRegistry};
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
        .expect_err("invalid arguments must fail validation");

    assert!(error.to_string().contains("string"));
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
        .expect_err("duplicate registration must fail");

    assert!(error.to_string().contains("already registered"));
}

#[test]
fn registry_enforces_draft_2020_12_string_combinator_and_object_constraints() {
    let mut registry = ToolRegistry::new();
    registry
        .register(ToolDefinition::new(
            "constrained",
            "A schema exercising standard JSON Schema keywords.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "minLength": 3},
                    "choice": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
                },
                "required": ["name", "choice"],
                "additionalProperties": false
            }),
        ))
        .expect("register constrained test schema");

    assert!(
        registry
            .validate_arguments("constrained", &json!({"name": "ab", "choice": "ok"}))
            .is_err()
    );
    assert!(
        registry
            .validate_arguments("constrained", &json!({"name": "valid", "choice": true}))
            .is_err()
    );
    assert!(
        registry
            .validate_arguments(
                "constrained",
                &json!({"name": "valid", "choice": "ok", "unexpected": true})
            )
            .is_err()
    );
    assert!(
        registry
            .validate_arguments("constrained", &json!({"name": "valid", "choice": 7}))
            .is_ok()
    );

    registry
        .register(ToolDefinition::new(
            "closed-object",
            "An object that accepts no properties.",
            json!({"type": "object", "additionalProperties": false}),
        ))
        .expect("register closed-object test schema");
    assert!(
        registry
            .validate_arguments("closed-object", &json!({"unexpected": true}))
            .is_err()
    );
}

#[test]
fn registry_rejects_malformed_schema_at_registration() {
    let mut registry = ToolRegistry::new();

    let error = registry
        .register(ToolDefinition::new(
            "malformed",
            "An invalid JSON Schema document.",
            json!({"type": "not-a-json-schema-type"}),
        ))
        .expect_err("malformed schema must fail registration");

    assert!(error.to_string().contains("invalid JSON Schema"));
}
