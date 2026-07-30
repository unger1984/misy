//! Incremental polling and PTY input for unified command sessions.

use super::{
    activity::{ActivityManager, InteractionError, output::DEFAULT_MAX_OUTPUT_TOKENS},
    command,
};
use crate::{ToolCall, ToolResult, activity::ActivityOwner};
use serde_json::Value;
use std::time::Duration;

const MIN_WRITE_YIELD_MS: u64 = 250;
const MAX_WRITE_YIELD_MS: u64 = 30_000;
const MIN_POLL_YIELD_MS: u64 = 5_000;
const MAX_POLL_YIELD_MS: u64 = 300_000;

pub(super) async fn execute(
    call: &ToolCall,
    owner: ActivityOwner,
    activities: &ActivityManager,
    cancellation: Option<tokio::sync::watch::Receiver<bool>>,
) -> ToolResult {
    let Some(task_id) = call.arguments.get("task_id").and_then(Value::as_str) else {
        return ToolResult::error(&call.id, "arguments.task_id must be a string");
    };
    let Some(id) = super::activity::parse_activity_id(task_id) else {
        return ToolResult::error(&call.id, "arguments.task_id must have the form `task-N`");
    };
    let chars = call
        .arguments
        .get("chars")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let wait = match yield_duration(call, chars.is_empty()) {
        Ok(wait) => wait,
        Err(message) => return ToolResult::error(&call.id, message),
    };
    let max_output_tokens = call
        .arguments
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_MAX_OUTPUT_TOKENS, |tokens| {
            usize::try_from(tokens).unwrap_or(usize::MAX)
        });
    match activities
        .interact_for_owner(owner, id, chars, wait, max_output_tokens, cancellation)
        .await
    {
        Ok(output) => command::result(call, &output),
        Err(error) => interaction_error(call, task_id, error),
    }
}

fn yield_duration(call: &ToolCall, empty: bool) -> Result<Duration, String> {
    let default = if empty {
        MIN_POLL_YIELD_MS
    } else {
        MIN_WRITE_YIELD_MS
    };
    let milliseconds = call
        .arguments
        .get("yield_time_ms")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| "arguments.yield_time_ms must be an integer".to_owned())
        })
        .transpose()?
        .unwrap_or(default);
    let (minimum, maximum) = if empty {
        (MIN_POLL_YIELD_MS, MAX_POLL_YIELD_MS)
    } else {
        (MIN_WRITE_YIELD_MS, MAX_WRITE_YIELD_MS)
    };
    if !(minimum..=maximum).contains(&milliseconds) {
        return Err(format!(
            "arguments.yield_time_ms must be between {minimum} and {maximum} for this interaction"
        ));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn interaction_error(call: &ToolCall, task_id: &str, error: InteractionError) -> ToolResult {
    let (kind, message) = match error {
        InteractionError::Unknown => ("unknown_task", format!("unknown task `{task_id}`")),
        InteractionError::StdinClosed => (
            "stdin_closed",
            "StdinClosed: this command uses closed pipe stdin; use empty chars to poll".to_owned(),
        ),
        InteractionError::Write(message) => (
            "stdin_write_error",
            format!("could not write command stdin: {message}"),
        ),
        InteractionError::Cancelled => ("cancelled", "write_stdin cancelled".to_owned()),
    };
    ToolResult::error(
        &call.id,
        serde_json::json!({"kind": kind, "message": message}).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_distinct_write_and_empty_poll_ranges() {
        let write = ToolCall::new("write", "write_stdin", json!({"yield_time_ms": 250}));
        let poll = ToolCall::new("poll", "write_stdin", json!({"yield_time_ms": 5000}));
        assert_eq!(
            yield_duration(&write, false),
            Ok(Duration::from_millis(250))
        );
        assert_eq!(yield_duration(&poll, true), Ok(Duration::from_millis(5000)));
        assert!(yield_duration(&write, true).is_err());
        assert!(yield_duration(&poll, false).is_ok());
        let too_long = ToolCall::new("long", "write_stdin", json!({"yield_time_ms": 300001}));
        assert!(yield_duration(&too_long, true).is_err());
    }
}
