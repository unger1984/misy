//! Public contracts and runtime services for misy.

pub mod config;
pub mod core;
pub mod domain;
pub mod providers;
pub mod tools;

pub use config::{Config, ConfigError, ConfigStore, CredentialError, CredentialStore, MisyPaths};
pub use core::{CoreError, CoreEvent, HistoryEntry, MisyCore, SubmissionId};
pub use domain::{
    Message, MessageRole, ModelId, ModelInfo, ModelRef, ProviderId, ToolCall, ToolDefinition,
    ToolResult,
};
pub use providers::{
    CHAT_CANCEL_REQUEST_ID_FIELD, PROVIDER_METHODS, PROVIDER_PROTOCOL_VERSION,
    PROVIDER_STREAM_EVENTS, PendingProviderRequest, ProviderCatalog, ProviderDiscoveryError,
    ProviderError, ProviderEvent, ProviderHost, ProviderManifest, ProviderPackage,
    ProviderRequestId,
};
pub use tools::{ToolDispatcher, ToolRegistry, ToolRegistryError};
