//! Forkable completed-history projection for child-agent sessions.

use crate::{HistoryEntry, Message, MessageRole};

/// Returns only completed parent entries, excluding the assistant entry that contains a spawn.
pub(crate) fn forkable_prefix(history: &[HistoryEntry]) -> Vec<HistoryEntry> {
    let completed_ids = history
        .iter()
        .flat_map(|entry| entry.tool_results.iter())
        .map(|result| &result.tool_call_id)
        .collect::<Vec<_>>();
    let boundary = history
        .iter()
        .position(|entry| {
            entry
                .tool_calls
                .iter()
                .any(|call| !completed_ids.contains(&&call.id))
        })
        .unwrap_or(history.len());
    history[..boundary].to_vec()
}

/// Builds an isolated child history from a completed parent prefix and one assignment.
pub(crate) fn child_history(parent_prefix: &[HistoryEntry], task: &str) -> Vec<HistoryEntry> {
    let mut history = Vec::with_capacity(parent_prefix.len().saturating_add(1));
    let removed_ids = parent_prefix
        .iter()
        .flat_map(|entry| entry.tool_calls.iter())
        .filter(|call| is_agent_tool(&call.name))
        .map(|call| call.id.clone())
        .collect::<Vec<_>>();
    history.extend(
        parent_prefix
            .iter()
            .cloned()
            .map(|entry| strip_agent_calls(entry, &removed_ids)),
    );
    history.retain(|entry| !is_empty_agent_entry(entry));
    history.push(HistoryEntry {
        message: Message::user(task),
        attachments: Vec::new(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        provider_metadata: serde_json::Value::Null,
    });
    history
}

fn strip_agent_calls(mut entry: HistoryEntry, removed_ids: &[String]) -> HistoryEntry {
    entry.tool_calls.retain(|call| !is_agent_tool(&call.name));
    entry
        .tool_results
        .retain(|result| !removed_ids.iter().any(|id| id == &result.tool_call_id));
    entry
}

fn is_empty_agent_entry(entry: &HistoryEntry) -> bool {
    entry.message.role == MessageRole::Assistant
        && entry.message.content.is_empty()
        && entry.tool_calls.is_empty()
        && entry.tool_results.is_empty()
}

fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent"
            | "agent_list"
            | "agent_wait"
            | "agent_output"
            | "agent_message"
            | "agent_stop"
    )
}

#[cfg(test)]
mod tests {
    use super::{child_history, forkable_prefix};
    use crate::{HistoryEntry, Message, MessageRole, ToolCall, ToolResult};
    use serde_json::Value;

    fn entry(role: MessageRole, calls: Vec<ToolCall>, results: Vec<ToolResult>) -> HistoryEntry {
        HistoryEntry {
            message: Message::new(role, ""),
            attachments: Vec::new(),
            tool_calls: calls,
            tool_results: results,
            provider_metadata: Value::Null,
        }
    }

    #[test]
    fn fork_removes_agent_pairs_but_preserves_mixed_local_tools() {
        let call = |id: &str, name: &str| ToolCall::new(id, name, Value::Null);
        let parent = vec![
            entry(
                MessageRole::Assistant,
                vec![call("agent", "spawn_agent"), call("read", "read_file")],
                Vec::new(),
            ),
            entry(
                MessageRole::Tool,
                Vec::new(),
                vec![
                    ToolResult::success("agent", "done"),
                    ToolResult::success("read", "text"),
                ],
            ),
        ];

        let child = child_history(&forkable_prefix(&parent), "inspect this");

        assert_eq!(child[0].tool_calls.len(), 1);
        assert_eq!(child[0].tool_calls[0].name, "read_file");
        assert_eq!(child[1].tool_results.len(), 1);
        assert_eq!(child[1].tool_results[0].tool_call_id, "read");
    }

    #[test]
    fn fork_excludes_current_incomplete_tool_call() {
        let parent = vec![entry(
            MessageRole::Assistant,
            vec![ToolCall::new("spawn", "spawn_agent", Value::Null)],
            Vec::new(),
        )];

        let child = child_history(&forkable_prefix(&parent), "inspect this");

        assert_eq!(child.len(), 1);
        assert_eq!(child[0].message.role, MessageRole::User);
    }
}
