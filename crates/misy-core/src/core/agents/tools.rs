//! Provider-visible child-agent tool definitions and presentation normalization.

use crate::ToolDefinition;

const MAX_AGENT_TITLE_CHARS: usize = 80;

pub(crate) fn tool_definitions() -> [ToolDefinition; 7] {
    [
        spawn_definition(),
        model_search_definition(),
        ToolDefinition::new(
            "agent_list",
            "List live and recent child agents owned by this conversation.",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        ),
        ToolDefinition::new(
            "agent_wait",
            "Wait for and consume matching background agent completions.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_ids": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 64,
                        "uniqueItems": true,
                        "items": {"type": "string", "pattern": "^agent-[1-9][0-9]*$"}
                    },
                    "timeout_ms": {
                        "type": "integer", "minimum": 250, "maximum": 300000,
                        "default": 10000
                    }
                },
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "agent_output",
            "Read a child agent's current bounded transcript and terminal result.",
            serde_json::json!({
                "type": "object",
                "required": ["agent_id"],
                "properties": {
                    "agent_id": {"type": "string", "minLength": 1},
                    "max_output_tokens": {
                        "type": "integer", "minimum": 1, "maximum": 1000000,
                        "default": 10000
                    }
                },
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "agent_message",
            "Queue a non-empty follow-up message for a live child agent's next model turn.",
            serde_json::json!({
                "type": "object",
                "required": ["agent_id", "message"],
                "properties": {
                    "agent_id": {"type": "string", "minLength": 1},
                    "message": {"type": "string", "minLength": 1}
                },
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "agent_stop",
            "Stop one live child agent without affecting Main or other agents.",
            serde_json::json!({
                "type": "object",
                "required": ["agent_id"],
                "properties": {
                    "agent_id": {"type": "string", "minLength": 1}
                },
                "additionalProperties": false
            }),
        ),
    ]
}

fn spawn_definition() -> ToolDefinition {
    ToolDefinition::new(
        "spawn_agent",
        "Run an independent child agent from a hot-reloaded role. A model string is strict; \
         an array is the complete ordered fallback chain. agent_type/model overrides require \
         fork_turns `none` or a positive integer.",
        serde_json::json!({
            "type": "object",
            "required": ["task", "task_name"],
            "properties": {
                "task": {"type": "string", "minLength": 1},
                "task_name": {"type": "string", "pattern": "^[a-z0-9_]+$"},
                "description": {"type": "string"},
                "run_in_background": {"type": "boolean", "default": false},
                "agent_type": {"type": "string", "minLength": 1},
                "fork_turns": {
                    "type": "string", "pattern": "^(none|all|[1-9][0-9]*)$", "default": "all"
                },
                "model": {"oneOf": [
                    {"type": "string", "minLength": 3},
                    {"type": "array", "minItems": 1, "uniqueItems": true,
                     "items": {"type": "string", "minLength": 3}}
                ]}
            },
            "additionalProperties": false
        }),
    )
}

fn model_search_definition() -> ToolDefinition {
    ToolDefinition::new(
        "model_search",
        "Search cached provider profiles, thinking levels, role membership, availability, and \
         freshness.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "maxLength": 256},
                "provider": {"type": "string", "maxLength": 64},
                "agent_type": {"type": "string", "maxLength": 64},
                "limit": {"type": "integer", "minimum": 1, "maximum": 20, "default": 8}
            },
            "additionalProperties": false
        }),
    )
}

pub(crate) fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent"
            | "agent_list"
            | "agent_wait"
            | "agent_output"
            | "agent_message"
            | "agent_stop"
            | "model_search"
    )
}

pub(crate) fn normalize_agent_title(description: Option<&str>, task: &str) -> String {
    let source = description
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| {
            task.lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(task)
        });
    let collapsed = strip_ansi(source)
        .chars()
        .map(|character| {
            if character.is_control() || character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut title = collapsed
        .chars()
        .take(MAX_AGENT_TITLE_CHARS)
        .collect::<String>();
    if collapsed.chars().count() > MAX_AGENT_TITLE_CHARS {
        title.pop();
        title.push('…');
    }
    if title.is_empty() {
        "Child agent".to_owned()
    } else {
        title
    }
}

fn strip_ansi(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        if characters.next() != Some('[') {
            continue;
        }
        for control in characters.by_ref() {
            if ('@'..='~').contains(&control) {
                break;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_one_bounded_display_safe_line() {
        let title = normalize_agent_title(Some("  review\n\u{1b}[31m queue\t races  "), "unused");
        assert_eq!(title, "review queue races");
        assert!(
            normalize_agent_title(None, &"x".repeat(200))
                .chars()
                .count()
                <= 80
        );
    }
}
