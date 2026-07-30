//! Domain types shared by the core, provider plugins, and clients. Every type here is
//! serializable because it crosses a process or client boundary, and the identifier newtypes
//! ([`ProviderId`], [`ModelId`]) keep provider and model identity from decaying into bare
//! strings that cannot be told apart at the type level.
use crate::{ImageAttachment, InputModality};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{error::Error, fmt, str::FromStr};

/// Stable identifier for a model provider.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
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

/// Provider name displayed to users; distinct from [`ProviderId`] so a map or struct cannot
/// silently hold one where the other belongs.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderDisplayName(String);

impl ProviderDisplayName {
    /// Creates a display name from an owned or borrowed string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the display name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderDisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<ProviderDisplayName> for String {
    fn from(name: ProviderDisplayName) -> Self {
        name.0
    }
}

/// Provider-scoped identifier for a model.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

/// One provider-owned reasoning level advertised by a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThinkingLevel {
    /// Stable provider-owned wire identifier.
    pub id: String,
    /// Bounded provider-supplied explanation of the level.
    pub description: String,
}

/// Provider-owned reasoning controls for one model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThinkingInfo {
    /// The level selected when a profile omits an explicit level.
    pub default: String,
    /// Ordered levels from lighter to deeper reasoning within this model.
    pub levels: Vec<ThinkingLevel>,
}

/// A model paired with an optional provider-owned reasoning level.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ModelProfile {
    /// Provider and provider-local model identity.
    pub model: ModelRef,
    /// Explicit reasoning level, or the provider default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl ModelProfile {
    /// Creates a profile from one model and an optional provider-owned reasoning level.
    pub fn new(model: ModelRef, thinking: Option<String>) -> Self {
        Self { model, thinking }
    }

    /// Returns the catalog-independent selector persisted in role and session data.
    pub fn selector(&self) -> String {
        let model = escape_model_component(self.model.model.as_str());
        match &self.thinking {
            Some(thinking) => format!("{}/{model}:{thinking}", self.model.provider.as_str()),
            None => format!("{}/{model}", self.model.provider.as_str()),
        }
    }
}

impl fmt::Display for ModelProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.selector())
    }
}

impl FromStr for ModelProfile {
    type Err = ModelSelectorError;

    /// Parses a catalog-independent `provider/model[:thinking]` selector.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (provider, rest) = value
            .split_once('/')
            .ok_or(ModelSelectorError::MissingProviderSeparator)?;
        if provider.is_empty() || rest.is_empty() {
            return Err(ModelSelectorError::EmptyComponent);
        }
        let (raw_model, thinking) = match rest.rsplit_once(':') {
            Some((_model, "")) => {
                return Err(ModelSelectorError::EmptyThinking);
            }
            Some((model, thinking)) => (model, Some(thinking)),
            None => (rest, None),
        };
        if raw_model.is_empty() {
            return Err(ModelSelectorError::EmptyComponent);
        }
        let model = unescape_model_component(raw_model)?;
        if model.is_empty() || thinking.is_some_and(|level| !valid_thinking_id(level)) {
            return Err(ModelSelectorError::InvalidComponent);
        }
        Ok(Self::new(
            ModelRef::new(ProviderId::new(provider), ModelId::new(model)),
            thinking.map(ToOwned::to_owned),
        ))
    }
}

/// Errors returned while parsing a persistent model selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSelectorError {
    /// The selector did not contain the required provider/model separator.
    MissingProviderSeparator,
    /// A required provider or model component was empty.
    EmptyComponent,
    /// A trailing `:` did not name a reasoning level.
    EmptyThinking,
    /// An escape or reasoning identifier was not valid for this contract.
    InvalidComponent,
}

impl fmt::Display for ModelSelectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingProviderSeparator => "model selector must contain provider/model",
            Self::EmptyComponent => "model selector contains an empty component",
            Self::EmptyThinking => "model selector has an empty thinking level",
            Self::InvalidComponent => "model selector contains an invalid escape or thinking level",
        })
    }
}

impl Error for ModelSelectorError {}

fn escape_model_component(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A")
}

fn unescape_model_component(value: &str) -> Result<String, ModelSelectorError> {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let escape = [characters.next(), characters.next()];
        match escape {
            [Some('2'), Some('5')] => output.push('%'),
            [Some('3'), Some('A' | 'a')] => output.push(':'),
            _ => return Err(ModelSelectorError::InvalidComponent),
        }
    }
    Ok(output)
}

fn valid_thinking_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && matches!(bytes.first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && bytes
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
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
    /// Optional bounded provider-supplied description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional opaque provider-supplied pricing text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<String>,
    /// Optional provider-owned reasoning controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingInfo>,
    /// Input modalities accepted by this provider model.
    #[serde(default = "text_input_modalities")]
    pub input_modalities: Vec<InputModality>,
}

impl ModelInfo {
    /// Creates provider-advertised model metadata.
    pub fn new(model: ModelRef, display_name: impl Into<String>, context_window: u32) -> Self {
        Self {
            model,
            display_name: display_name.into(),
            context_window,
            description: None,
            pricing: None,
            thinking: None,
            input_modalities: text_input_modalities(),
        }
    }

    /// Replaces the conservative text-only default with provider-advertised modalities.
    pub fn with_input_modalities(mut self, input_modalities: Vec<InputModality>) -> Self {
        self.input_modalities = input_modalities;
        self
    }

    /// Adds optional provider metadata without interpreting its text or reasoning vocabulary.
    pub fn with_optional_metadata(
        mut self,
        description: Option<String>,
        pricing: Option<String>,
        thinking: Option<ThinkingInfo>,
    ) -> Self {
        self.description = description;
        self.pricing = pricing;
        self.thinking = thinking;
        self
    }

    /// Reports whether this model accepts one normalized input modality.
    pub fn supports(&self, modality: InputModality) -> bool {
        self.input_modalities.contains(&modality)
    }
}

fn text_input_modalities() -> Vec<InputModality> {
    vec![InputModality::Text]
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
    /// Validated images returned to an image-capable model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ImageAttachment>,
    /// Whether the tool execution failed.
    pub is_error: bool,
}

impl ToolResult {
    /// Creates a successful tool result.
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            attachments: Vec::new(),
            is_error: false,
        }
    }

    /// Creates a failed tool result.
    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            attachments: Vec::new(),
            is_error: true,
        }
    }

    /// Creates a successful result containing text and validated images.
    pub fn success_with_attachments(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        attachments: Vec<ImageAttachment>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            attachments,
            is_error: false,
        }
    }
}
