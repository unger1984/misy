//! Provider-visible definitions for command and filesystem tools.

use super::image_view;
use crate::ToolDefinition;
use serde_json::Value;

pub(super) fn builtin_definitions() -> Vec<ToolDefinition> {
    let mut definitions = vec![
        crate::core::questions::definition(),
        crate::core::todos::definition(),
        ToolDefinition::new(
            "list_directory",
            "List entries in a directory.",
            serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {"path": {"type": "string"}},
                "additionalProperties": false,
            }),
        ),
        read_file_definition(),
        image_view::definition(),
        exec_command_definition(),
        ToolDefinition::new(
            "task_list",
            "List active and recent background tasks.",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        ),
        ToolDefinition::new(
            "task_stop",
            "Stop an active background task and its child processes.",
            serde_json::json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string", "pattern": "^task-[1-9][0-9]*$"}
                },
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "write_stdin",
            "Poll new output from any live exec_command session, or write to a PTY session. \
             Empty chars only polls and never closes stdin. No completion notification is sent; \
             keep polling until exit_code is returned. Non-empty input to a pipe session returns \
             StdinClosed.",
            serde_json::json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string", "pattern": "^task-[1-9][0-9]*$"},
                    "chars": {"type": "string", "default": ""},
                    "yield_time_ms": {
                        "type": "integer", "minimum": 250, "maximum": 300000
                    },
                    "max_output_tokens": {
                        "type": "integer", "minimum": 1, "maximum": 1000000, "default": 10000
                    }
                },
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "write_file",
            "Write UTF-8 content to a file.",
            serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "StrReplaceFile",
            "Replace an exact UTF-8 fragment in an existing file. By default, the fragment must \
             occur exactly once; set replace_all to replace every non-overlapping occurrence. \
             Input and output are limited to 4 MiB.",
            serde_json::json!({
                "type": "object",
                "required": ["path", "old", "new"],
                "properties": {
                    "path": {"type": "string"},
                    "old": {"type": "string"},
                    "new": {"type": "string"},
                    "replace_all": {"type": "boolean", "default": false},
                },
                "additionalProperties": false,
            }),
        ),
    ];
    definitions.sort_by(|left, right| left.name.cmp(&right.name));
    definitions
}

fn read_file_definition() -> ToolDefinition {
    ToolDefinition::new(
        "read_file",
        "Read a bounded UTF-8 file. Optionally use one-based offset and limit to read a \
         contiguous line page; omit both for the existing whole-file result.",
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer", "minimum": 1},
                "limit": {"type": "integer", "minimum": 1, "maximum": 1000},
            },
            "additionalProperties": false,
        }),
    )
}

fn exec_command_definition() -> ToolDefinition {
    ToolDefinition::new(
        "exec_command",
        "Run a shell command. Fast commands return inline; long commands continue as sessions. \
         No completion notification is sent to the model, so poll a returned task_id with an \
         empty write_stdin until exit_code is present. PTY mode is supported on macOS and Linux.",
        serde_json::json!({
            "type": "object",
            "required": ["cmd"],
            "properties": {
                "cmd": {"type": "string"},
                "cwd": {"type": "string"},
                "description": {"type": "string"},
                "run_in_background": {"type": "boolean", "default": false},
                "yield_time_ms": command_yield_schema(),
                "timeout_seconds": command_timeout_schema(),
                "tty": {"type": "boolean", "default": false},
                "max_output_tokens": {
                    "type": "integer", "minimum": 1, "maximum": 1000000, "default": 10000
                }
            },
            "additionalProperties": false,
        }),
    )
}

fn command_yield_schema() -> Value {
    serde_json::json!({
        "type": "integer", "minimum": 250, "maximum": 30000, "default": 10000
    })
}

fn command_timeout_schema() -> Value {
    serde_json::json!({
        "type": "integer", "minimum": 0, "maximum": 86400, "default": 0
    })
}
