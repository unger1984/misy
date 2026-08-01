//! Exact, bounded UTF-8 file replacement for model-issued tool calls.

use super::filesystem::{self, MAX_TEXT_FILE_BYTES};
use crate::{ToolCall, ToolResult};
use serde_json::Value;
use std::fs;
use tokio::task::spawn_blocking;

pub(super) async fn execute(call: &ToolCall) -> ToolResult {
    let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
        return ToolResult::error(&call.id, "arguments.path must be a string");
    };
    let Some(old) = call.arguments.get("old").and_then(Value::as_str) else {
        return ToolResult::error(&call.id, "arguments.old must be a string");
    };
    let Some(new) = call.arguments.get("new").and_then(Value::as_str) else {
        return ToolResult::error(&call.id, "arguments.new must be a string");
    };
    let replace_all = call
        .arguments
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let path = path.to_owned();
    let old = old.to_owned();
    let new = new.to_owned();
    match spawn_blocking({
        let path = path.clone();
        move || replace(&path, &old, &new, replace_all)
    })
    .await
    {
        Ok(Ok(count)) => {
            let noun = if count == 1 {
                "replacement"
            } else {
                "replacements"
            };
            ToolResult::success(&call.id, format!("replaced {count} {noun} in {path}"))
        }
        Ok(Err(error)) => ToolResult::error(&call.id, error),
        Err(error) => ToolResult::error(
            &call.id,
            format!("could not replace in {path}: filesystem task failed: {error}"),
        ),
    }
}

fn replace(path: &str, old: &str, new: &str, replace_all: bool) -> Result<usize, String> {
    validate_fragments(old, new)?;
    let content = filesystem::read_complete_utf8_file(path)?;
    let count = content.match_indices(old).count();
    validate_match_count(count, replace_all)?;
    let projected_len = projected_length(content.len(), count, old.len(), new.len())?;
    if projected_len > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "replacement result exceeds the {MAX_TEXT_FILE_BYTES}-byte limit"
        ));
    }
    let replacement = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    fs::write(path, replacement).map_err(|error| format!("could not write {path}: {error}"))?;
    Ok(count)
}

fn validate_fragments(old: &str, new: &str) -> Result<(), String> {
    if old.is_empty() {
        return Err("old must not be empty".to_owned());
    }
    if old == new {
        return Err("old and new must differ".to_owned());
    }
    Ok(())
}

fn validate_match_count(count: usize, replace_all: bool) -> Result<(), String> {
    if count == 0 {
        return Err("old was not found in the target file".to_owned());
    }
    if count > 1 && !replace_all {
        return Err(
            "old occurs more than once; provide more identifying context or set replace_all"
                .to_owned(),
        );
    }
    Ok(())
}

fn projected_length(
    input_len: usize,
    matches: usize,
    old_len: usize,
    new_len: usize,
) -> Result<usize, String> {
    let removed = matches
        .checked_mul(old_len)
        .ok_or_else(|| "replacement result length overflowed".to_owned())?;
    let remaining = input_len
        .checked_sub(removed)
        .ok_or_else(|| "replacement result length overflowed".to_owned())?;
    let added = matches
        .checked_mul(new_len)
        .ok_or_else(|| "replacement result length overflowed".to_owned())?;
    remaining
        .checked_add(added)
        .ok_or_else(|| "replacement result length overflowed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::projected_length;

    #[test]
    fn projected_length_rejects_synthetic_overflow() {
        assert!(projected_length(1, usize::MAX, 1, 2).is_err());
    }
}
