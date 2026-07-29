//! Image-file tool definition and bounded execution.

use crate::{ImageAttachment, ToolCall, ToolDefinition, ToolResult};
use serde_json::json;
use tokio::task::spawn_blocking;

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition::new(
        "view_image",
        "View a local image file from the filesystem when visual inspection is needed.",
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}},
            "additionalProperties": false,
        }),
    )
}

pub(super) async fn execute(call: &ToolCall) -> ToolResult {
    let Some(path) = call
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
    else {
        return ToolResult::error(&call.id, "arguments.path must be a string");
    };
    let path = path.to_owned();
    match spawn_blocking({
        let path = path.clone();
        move || ImageAttachment::from_path(&path)
    })
    .await
    {
        Ok(Ok(image)) => ToolResult::success_with_attachments(
            &call.id,
            format!("Read image file `{path}` [{}]", image.media_type()),
            vec![image],
        ),
        Ok(Err(error)) => ToolResult::error(&call.id, format!("could not view {path}: {error}")),
        Err(error) => ToolResult::error(
            &call.id,
            format!("could not view {path}: filesystem task failed: {error}"),
        ),
    }
}
