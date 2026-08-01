//! Tool-registry integration tests.

use misy_core::{ToolDefinition, ToolRegistry};
use serde_json::json;

#[test]
fn registry_exposes_the_builtin_tool_definitions() {
    let registry = ToolRegistry::new();
    let definitions = registry.definitions();
    let names: Vec<_> = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "AskUserQuestion",
            "SetTodoList",
            "StrReplaceFile",
            "agent_list",
            "agent_message",
            "agent_output",
            "agent_stop",
            "agent_wait",
            "exec_command",
            "list_directory",
            "model_search",
            "read_file",
            "spawn_agent",
            "task_list",
            "task_stop",
            "view_image",
            "write_file",
            "write_stdin"
        ]
    );
}

#[test]
fn registry_exposes_the_string_replacement_contract() {
    let registry = ToolRegistry::new();
    let definition = registry
        .get("StrReplaceFile")
        .expect("string replacement definition");

    assert_eq!(definition.name, "StrReplaceFile");
    assert!(
        registry
            .validate_arguments(
                "StrReplaceFile",
                &json!({"path": "note.txt", "old": "before", "new": "after"}),
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_arguments(
                "StrReplaceFile",
                &json!({"path": "note.txt", "old": "before", "new": "after", "replace_all": true}),
            )
            .is_ok()
    );
    for invalid in [
        json!({"path": "note.txt", "old": "before"}),
        json!({"path": "note.txt", "old": 1, "new": "after"}),
        json!({"path": "note.txt", "old": "before", "new": "after", "replace_all": "yes"}),
        json!({"path": "note.txt", "old": "before", "new": "after", "extra": true}),
    ] {
        assert!(
            registry
                .validate_arguments("StrReplaceFile", &invalid)
                .is_err()
        );
    }
}

#[test]
fn kimi_tool_schemas_reject_unknown_fields_and_invalid_shapes() {
    let registry = ToolRegistry::new();
    assert!(
        registry
            .validate_arguments(
                "SetTodoList",
                &json!({"todos": [{"title": "Work", "status": "pending"}]})
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_arguments("SetTodoList", &json!({"todos": [], "merge": true}))
            .is_err()
    );
    assert!(
        registry
            .validate_arguments(
                "AskUserQuestion",
                &json!({"questions": [{
                    "question": "Choose",
                    "options": [{"label": "A"}, {"label": "B"}],
                    "unexpected": true
                }]})
            )
            .is_err()
    );
}

#[test]
fn registry_accepts_only_the_unified_command_contract() {
    let registry = ToolRegistry::new();
    assert!(
        registry
            .validate_arguments("exec_command", &json!({"cmd": "printf ok"}))
            .is_ok()
    );
    assert!(
        registry
            .validate_arguments(
                "exec_command",
                &json!({"cmd": "printf ok", "yield_time_ms": 250})
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_arguments(
                "exec_command",
                &json!({"cmd": "printf ok", "yield-time-ms": 250})
            )
            .is_err()
    );
    for legacy in ["run_command", "run_shell", "shell_command", "task_output"] {
        assert!(registry.validate_arguments(legacy, &json!({})).is_err());
    }
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
