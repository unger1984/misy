//! Checklist contracts and validation for the core-owned `SetTodoList` tool.

use super::{
    CoreEvent, CoreState,
    turn::{AgentTurnState, PersistencePolicy},
};
use crate::{ToolCall, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};

/// Maximum number of checklist items accepted by version one.
pub const MAX_TODO_ITEMS: usize = 64;
/// Maximum Unicode scalar values in one checklist title.
pub const MAX_TODO_TITLE_CHARS: usize = 200;
/// Maximum aggregate UTF-8 bytes in checklist titles.
pub const MAX_TODO_TEXT_BYTES: usize = 16 * 1024;
/// Maximum encoded checklist argument payload.
pub const MAX_TODO_SERIALIZED_BYTES: usize = 32 * 1024;
/// Stable success result for a checklist replacement.
pub const TODO_UPDATED_RESULT: &str = "Todo list updated";
/// Stable result for an empty checklist query.
pub const TODO_EMPTY_RESULT: &str = "Todo list is empty.";

/// One allowed checklist status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Work not yet started.
    Pending,
    /// Work currently underway.
    InProgress,
    /// Completed work.
    Done,
}

impl TodoStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

/// An ordered item in a session checklist.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    /// Display title, retained exactly as supplied after validation.
    pub title: String,
    /// Current checklist status.
    pub status: TodoStatus,
}

/// Provider-visible definition for the Kimi-compatible checklist tool.
pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition::new(
        "SetTodoList",
        "Replace the current session todo list, or read it when todos is omitted.",
        serde_json::json!({
            "type": "object",
            "properties": {"todos": {"type": ["array", "null"], "maxItems": MAX_TODO_ITEMS,
                "items": {"type": "object", "required": ["title", "status"],
                    "properties": {
                        "title": {"type": "string"},
                        "status": {"enum": ["pending", "in_progress", "done"]}
                    },
                    "additionalProperties": false}}},
            "additionalProperties": false
        }),
    )
}

/// Validates an already schema-valid replacement payload.
pub(crate) fn validate(todos: &[TodoItem]) -> Result<(), String> {
    if todos.len() > MAX_TODO_ITEMS {
        return Err(format!("todos contains more than {MAX_TODO_ITEMS} items"));
    }
    let bytes = todos.iter().try_fold(0_usize, |total, todo| {
        if todo.title.trim().is_empty() || todo.title.trim().chars().count() > MAX_TODO_TITLE_CHARS
        {
            return Err(format!(
                "todo title must be non-empty and at most {MAX_TODO_TITLE_CHARS} characters"
            ));
        }
        Ok::<_, String>(total.saturating_add(todo.title.len()))
    })?;
    if bytes > MAX_TODO_TEXT_BYTES {
        return Err(format!("todo titles exceed {MAX_TODO_TEXT_BYTES} bytes"));
    }
    if serde_json::to_vec(&serde_json::json!({ "todos": todos }))
        .map_err(|error| error.to_string())?
        .len()
        > MAX_TODO_SERIALIZED_BYTES
    {
        return Err(format!(
            "todo payload exceeds {MAX_TODO_SERIALIZED_BYTES} bytes"
        ));
    }
    Ok(())
}

/// Formats a checklist query result with the provider-visible stable wording.
pub(crate) fn format_query(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return TODO_EMPTY_RESULT.to_owned();
    }
    let mut result = String::from("Current todo list:");
    for todo in todos {
        result.push_str(&format!("\n- [{}] {}", todo.status.label(), todo.title));
    }
    result
}

pub(crate) fn dispatch(core: &CoreState, state: &AgentTurnState, call: &ToolCall) -> ToolResult {
    let Some(value) = call.arguments.get("todos") else {
        return query(state, call);
    };
    if value.is_null() {
        return query(state, call);
    }
    let todos: Vec<TodoItem> = match serde_json::from_value(value.clone()) {
        Ok(todos) => todos,
        Err(error) => return ToolResult::error(&call.id, error.to_string()),
    };
    if let Err(error) = validate(&todos) {
        return ToolResult::error(&call.id, error);
    }
    *state
        .todos()
        .lock()
        .expect("todo mutex must not be poisoned") = todos.clone();
    if state.persistence() == PersistencePolicy::Conversation {
        core.persist_todos(todos.clone());
        core.emit(&CoreEvent::TodoListUpdated { todos });
    }
    ToolResult::success(&call.id, TODO_UPDATED_RESULT)
}

fn query(state: &AgentTurnState, call: &ToolCall) -> ToolResult {
    let todos = state
        .todos()
        .lock()
        .expect("todo mutex must not be poisoned")
        .clone();
    ToolResult::success(&call.id, format_query(&todos))
}
