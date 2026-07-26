//! Public contracts and runtime services for Misy.
//!
//! Construct [`MisyCore`] with provider packages discovered by [`ProviderCatalog`], then use its
//! event stream and operations to build a client without owning agent state.

/// Normalized provider account-limit reports.
#[path = "usage.rs"]
pub mod account_limits;
/// Versioned local configuration and opaque credential persistence.
pub mod config;
/// Headless orchestration, sessions, and normalized core events.
pub mod core;
/// Versioned values exchanged across core, provider, and tool boundaries.
pub mod domain;
/// Persistent, best-effort cache of provider model catalogs.
pub mod model_cache;
/// Provider package discovery and JSON-RPC subprocess supervision.
pub mod providers;
/// Local tool definitions, validation, and execution.
pub mod tools;

pub use account_limits::{
    UsageAmount, UsageLimit, UsageReport, UsageStatus, UsageUnit, UsageWindow,
};
pub use config::{Config, ConfigError, ConfigStore, CredentialError, CredentialStore, MisyPaths};
pub use core::{
    AvailableModels, CoreError, CoreEvent, CoreSnapshot, HistoryEntry, MisyCore, ProviderAuthState,
    ProviderModelError, SubmissionId,
};
pub use domain::{
    Message, MessageRole, ModelId, ModelInfo, ModelRef, ProviderId, ToolCall, ToolDefinition,
    ToolResult,
};
pub use model_cache::{ModelCatalogError, ModelCatalogStore};
pub use providers::{
    CHAT_CANCEL_REQUEST_ID_FIELD, PROVIDER_METHODS, PROVIDER_PROTOCOL_VERSION,
    PROVIDER_STREAM_EVENTS, PendingProviderRequest, ProviderAuthMethod, ProviderCapability,
    ProviderCatalog, ProviderDiscoveryError, ProviderError, ProviderEvent, ProviderHost,
    ProviderManifest, ProviderPackage, ProviderRequestId, USAGE_CAPABILITY,
    USAGE_CAPABILITY_VERSION, USAGE_METHOD,
};
pub use tools::{ToolDispatcher, ToolRegistry, ToolRegistryError};
