//! Public contracts and runtime services for Misy.
//!
//! Construct [`MisyCore`] with provider packages discovered by [`ProviderCatalog`], then use its
//! event stream and operations to build a client without owning agent state.

/// Versioned local configuration and opaque credential persistence.
pub mod config;
/// Headless orchestration, sessions, and normalized core events.
pub mod core;
/// Versioned values exchanged across core, provider, and tool boundaries.
pub mod domain;
/// Provider package discovery and JSON-RPC subprocess supervision.
pub mod providers;
/// Local tool definitions, validation, and execution.
pub mod tools;
/// Terminal client for the headless core.
pub mod tui;

pub use config::{Config, ConfigError, ConfigStore, CredentialError, CredentialStore, MisyPaths};
pub use core::{
    AvailableModels, CoreError, CoreEvent, HistoryEntry, MisyCore, ProviderModelError, SubmissionId,
};
pub use domain::{
    Message, MessageRole, ModelId, ModelInfo, ModelRef, ProviderId, ToolCall, ToolDefinition,
    ToolResult,
};
pub use providers::{
    CHAT_CANCEL_REQUEST_ID_FIELD, PROVIDER_METHODS, PROVIDER_PROTOCOL_VERSION,
    PROVIDER_STREAM_EVENTS, PendingProviderRequest, ProviderAuthMethod, ProviderCatalog,
    ProviderDiscoveryError, ProviderError, ProviderEvent, ProviderHost, ProviderManifest,
    ProviderPackage, ProviderRequestId,
};
pub use tools::{ToolDispatcher, ToolRegistry, ToolRegistryError};
