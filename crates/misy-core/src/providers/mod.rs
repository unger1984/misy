//! Provider hosting boundary: discovery of plugin packages, the JSON-RPC wire protocol they
//! speak, and the subprocess host that drives them. This file is a table of contents only; it
//! re-exports the contract the rest of the core and the clients are written against.
mod host;
mod manifest;
mod protocol;
mod redaction;

pub use host::{PendingProviderRequest, ProviderDeadlines, ProviderHost};
pub use manifest::{
    ProviderAuthMethod, ProviderCapability, ProviderCatalog, ProviderDiscoveryError,
    ProviderManifest, ProviderPackage,
};
pub use protocol::{
    CHAT_CANCEL_REQUEST_ID_FIELD, PROVIDER_METHODS, PROVIDER_PROTOCOL_VERSION,
    PROVIDER_STREAM_EVENTS, ProviderError, ProviderEvent, ProviderRequestId, USAGE_CAPABILITY,
    USAGE_CAPABILITY_VERSION, USAGE_METHOD,
};
