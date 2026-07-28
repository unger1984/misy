//! Public contracts and runtime services for Misy.
//!
//! Construct [`MisyCore`] with [`MisyCore::discover`], then use its event stream and operations
//! to build a client without owning agent state. Modules stay crate-private so the client
//! contract is exactly the set of names re-exported here.
//!
//! # Client walkthrough
//!
//! This sequence is the compiled contract of an external client: it deliberately touches only
//! the public surface, without the `test-support` feature, so breaking any of these signatures
//! fails `cargo test --doc`. It is `no_run` because running it would discover and launch real
//! provider plugins.
//!
//! ```no_run
//! use misy_core::{CoreEvent, Message, MisyCore, MisyPaths};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Discovery starts the core's private runtime without launching provider processes.
//!     let paths = MisyPaths::from_home()?;
//!     let core = MisyCore::discover(paths, "plugins/providers")?;
//!
//!     // Subscribe before submitting so no event is missed. The stream is lossy for slow
//!     // listeners, so the cheap snapshot stays the projection of core-owned state.
//!     let mut events = core.subscribe();
//!     println!("selected model: {:?}", core.snapshot().selected_model);
//!
//!     // A submission requires a selected model and enters the core-owned FIFO queue.
//!     let submission = core.submit(Message::user("Summarize README.md")).await?;
//!
//!     while let Some(event) = events.recv().await {
//!         match event {
//!             CoreEvent::TextDelta { delta, .. } => print!("{delta}"),
//!             CoreEvent::Completed { submission: done } if done == submission => break,
//!             CoreEvent::Failed { message, .. } => eprintln!("failed: {message}"),
//!             CoreEvent::Shutdown => break,
//!             _ => {}
//!         }
//!     }
//!
//!     // Shutdown drains provider processes; dropping the core without it is also safe.
//!     core.shutdown().await?;
//!     Ok(())
//! }
//! ```

/// Normalized provider account-limit reports.
pub(crate) mod account_limits;
/// Versioned local configuration and opaque credential persistence.
pub(crate) mod config;
/// Headless orchestration, sessions, and normalized core events.
pub(crate) mod core;
/// Versioned values exchanged across core, provider, and tool boundaries.
pub(crate) mod domain;
/// Lossy/lossless event fan-out shared by the core bus and the provider host.
pub(crate) mod fanout;
/// Persistent, best-effort cache of provider model catalogs.
pub(crate) mod model_cache;
/// Provider package discovery and JSON-RPC subprocess supervision.
pub(crate) mod providers;
/// Local tool definitions, validation, and execution.
pub(crate) mod tools;

pub use account_limits::{
    UsageAmount, UsageLimit, UsageReport, UsageStatus, UsageUnit, UsageWindow,
};
pub use config::{Config, ConfigError, ConfigStore, CredentialError, MisyPaths};
pub use core::{
    AvailableModels, CoreError, CoreEvent, CoreSnapshot, HistoryEntry, MisyCore, ProviderAuthState,
    ProviderModelError, SubmissionId,
};
pub use domain::{
    Message, MessageRole, ModelId, ModelInfo, ModelRef, ProviderDisplayName, ProviderId, ToolCall,
    ToolDefinition, ToolResult,
};
pub use providers::{
    CHAT_CANCEL_REQUEST_ID_FIELD, PROVIDER_METHODS, PROVIDER_PROTOCOL_VERSION,
    PROVIDER_STREAM_EVENTS, ProviderAuthMethod, ProviderCapability, ProviderDiscoveryError,
    ProviderError, ProviderManifest, ProviderPackage, ProviderRequestId, USAGE_CAPABILITY,
    USAGE_CAPABILITY_VERSION, USAGE_METHOD,
};

// Integration tests exercise these internals through the crate boundary. They are not part of
// the client contract: a client hosting its own `ProviderHost` would bypass the core-owned FIFO
// queue, credential injection, and event normalization.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use config::CredentialStore;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use providers::{
    PendingProviderRequest, ProviderCatalog, ProviderDeadlines, ProviderEvent, ProviderHost,
};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use tools::{ToolDispatcher, ToolRegistry, ToolRegistryError};
