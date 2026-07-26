use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable identifier for a model provider.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    /// Creates a provider identifier from an owned or borrowed string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider-scoped identifier for a model.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    /// Creates a model identifier from an owned or borrowed string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifies one model without relying on a process-wide provider selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRef {
    /// Provider that owns the model.
    pub provider: ProviderId,
    /// Provider-local model identifier.
    pub model: ModelId,
}

impl ModelRef {
    /// Combines a provider and its model identifier.
    pub fn new(provider: ProviderId, model: ModelId) -> Self {
        Self { provider, model }
    }
}

/// Provider-advertised metadata for a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelInfo {
    /// Stable reference used in requests.
    pub model: ModelRef,
    /// Human-readable model name.
    pub display_name: String,
    /// Maximum context size advertised by the provider.
    pub context_window: u32,
}

impl ModelInfo {
    /// Creates provider-advertised model metadata.
    pub fn new(model: ModelRef, display_name: impl Into<String>, context_window: u32) -> Self {
        Self {
            model,
            display_name: display_name.into(),
            context_window,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Role assigned to a normalized chat message.
pub enum MessageRole {
    /// Provider-defined instructions.
    System,
    /// End-user input.
    User,
    /// Model-generated content.
    Assistant,
    /// Local tool results sent back to the model.
    Tool,
}

/// A normalized chat message exchanged between the core and a provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Message {
    /// The semantic role of this message.
    pub role: MessageRole,
    /// UTF-8 message contents.
    pub content: String,
}

impl Message {
    /// Creates a message with an explicit role.
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    /// Creates an end-user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, content)
    }
}

/// A function exposed to a provider using a JSON Schema argument contract.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolDefinition {
    /// Stable provider-visible tool name.
    pub name: String,
    /// Human-readable purpose shown to the provider.
    pub description: String,
    /// JSON Schema validating the tool arguments.
    pub input_schema: Value,
}

impl ToolDefinition {
    /// Creates a tool definition from its provider-visible contract.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// A provider request to invoke one registered tool.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolCall {
    /// Provider-assigned call identifier.
    pub id: String,
    /// Registered tool name.
    pub name: String,
    /// JSON arguments supplied by the provider.
    pub arguments: Value,
}

impl ToolCall {
    /// Creates one provider-originated tool call.
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// The serializable outcome of a tool call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolResult {
    /// Identifier of the tool call this result completes.
    pub tool_call_id: String,
    /// UTF-8 result contents.
    pub content: String,
    /// Whether the tool execution failed.
    pub is_error: bool,
}

impl ToolResult {
    /// Creates a successful tool result.
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// Creates a failed tool result.
    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}
