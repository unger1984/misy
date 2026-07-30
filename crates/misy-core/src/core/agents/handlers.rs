//! Child-agent lookup, mailbox, messaging, output, and stop handlers.

use super::{AgentId, MailboxWait};
use crate::{
    ToolCall,
    core::{CoreState, turn},
};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_WAIT_MS: u64 = 10_000;
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;

pub(super) async fn wait(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let ids: Option<Vec<AgentId>> = call
        .arguments
        .get("agent_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| parse_agent_value(core, parent, value))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let timeout_ms = call
        .arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WAIT_MS);
    let outcome = core
        .agents
        .wait_mailbox(
            ids.as_deref(),
            Duration::from_millis(timeout_ms),
            parent.active().cancellation_receiver(),
        )
        .await
        .map_err(|error| error.to_string())?;
    match outcome {
        MailboxWait::Ready(agents) => encode(json!({"timed_out": false, "agents": agents})),
        MailboxWait::TimedOut => encode(json!({"timed_out": true, "agents": []})),
        MailboxWait::Cancelled => Err("agent_wait cancelled".to_owned()),
    }
}

pub(super) fn output(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let id = parse_agent_argument(core, parent, call)?;
    let transcript = core
        .agents
        .transcript(id)
        .map_err(|error| error.to_string())?;
    let maximum = call
        .arguments
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
        .saturating_mul(4);
    let text = transcript
        .entries
        .iter()
        .map(|entry| format!("{:?}: {}", entry.kind, entry.content))
        .collect::<Vec<_>>()
        .join("\n");
    encode(json!({
        "agent": transcript.agent,
        "transcript": project_text(&text, maximum),
        "truncated": transcript.truncated || text.len() > maximum,
    }))
}

pub(super) fn message(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let id = parse_agent_argument(core, parent, call)?;
    let message = call
        .arguments
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .ok_or_else(|| "arguments.message must not be empty".to_owned())?;
    core.agents
        .message(id, message)
        .map_err(|error| error.to_string())?;
    encode(json!({"agent_id": id, "status": "accepted"}))
}

pub(super) fn stop(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let id = parse_agent_argument(core, parent, call)?;
    core.agents
        .stop_tree(id)
        .map_err(|error| error.to_string())?;
    encode(json!({"agent_id": id, "status": "stopping"}))
}

fn parse_agent_argument(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<AgentId, String> {
    call.arguments
        .get("agent_id")
        .map(|value| parse_agent_value(core, parent, value))
        .transpose()?
        .ok_or_else(|| "arguments.agent_id is required".to_owned())
}

fn parse_agent_value(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    value: &Value,
) -> Result<AgentId, String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "agent_id must be a string".to_owned())?;
    let caller = match parent.identity() {
        turn::AgentTurnIdentity::Main => None,
        turn::AgentTurnIdentity::Child(id) => Some(id),
    };
    core.agents.resolve_target(caller, raw)
}

fn project_text(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_owned();
    }
    let half = maximum.saturating_sub(64) / 2;
    let head = text.chars().take(half).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{head}\n[... output omitted ...]\n{tail}")
}

fn encode(value: impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(&value).map_err(|error| format!("could not encode agent result: {error}"))
}
