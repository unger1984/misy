//! Versioned JSON-RPC contract shared by the provider host and provider packages.
//!
//! `auth.start` returns a discriminated union selected by `kind`:
//!
//! ```text
//! { "kind": "browser", "url": "https://…", "session": {…} }
//! { "kind": "device", "url": "https://…", "user_code": "WDJB-MJHT",
//!   "expires_at": 1795000000000, "session": {…} }
//! { "kind": "prompt", "fields": [
//!   { "id": "api_key", "label": "API key", "secret": true }
//! ], "session": {…} }
//! { "kind": "none" }
//! ```
//!
//! `kind` is required and unknown values are rejected by clients. `url` is required for browser
//! and device flows. A device URL is `verification_uri_complete` when available and otherwise
//! `verification_uri`; `user_code` is also required. `expires_at` is an optional Unix epoch time
//! in milliseconds. Prompt flows require a non-empty `fields` array, and `secret` fields must be
//! masked by supporting clients. `session` is an opaque authentication-attempt marker required
//! for every flow except `none`.
//!
//! `auth.complete` receives separate `session` and `completion` fields. Browser and device flows
//! use an empty completion object. Adding an optional result field is backward compatible. Adding
//! or renaming a required field, method, or event requires a protocol version change.

use crate::ProviderId;
use serde_json::Value;
use std::{error::Error, fmt};

/// The JSON-RPC protocol revision implemented by this host.
pub const PROVIDER_PROTOCOL_VERSION: u32 = 2;

/// Optional account-limit reporting capability identifier.
pub const USAGE_CAPABILITY: &str = "usage";

/// Usage capability contract revision supported by this host.
pub const USAGE_CAPABILITY_VERSION: u32 = 1;

/// JSON-RPC method required from providers advertising usage capability version 1.
pub const USAGE_METHOD: &str = "usage.get";

/// Optional multimodal chat and tool-result capability identifier.
pub const IMAGE_INPUT_CAPABILITY: &str = "image_input";

/// Image-input capability contract revision supported by this host.
pub const IMAGE_INPUT_CAPABILITY_VERSION: u32 = 1;
/// Optional provider-owned reasoning control capability identifier.
pub const THINKING_CAPABILITY: &str = "thinking";
/// Thinking capability contract revision supported by this host.
pub const THINKING_CAPABILITY_VERSION: u32 = 1;
/// Maximum encoded size of one newline-delimited provider protocol frame.
pub(crate) const MAX_PROTOCOL_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// Methods every version 2 provider package must implement.
pub const PROVIDER_METHODS: &[&str] = &[
    "auth.status",
    "auth.start",
    "auth.complete",
    "auth.refresh",
    "auth.logout",
    "models.list",
    "chat.start",
    "chat.cancel",
];

/// Notification methods emitted while a provider streams a chat turn.
pub const PROVIDER_STREAM_EVENTS: &[&str] = &["text_delta", "tool_call", "completed", "failed"];

/// Payload key used by `chat.cancel` to identify the `chat.start` request being cancelled.
pub const CHAT_CANCEL_REQUEST_ID_FIELD: &str = "request_id";

/// A JSON-RPC notification received from a provider subprocess.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderEvent {
    /// Provider process that emitted this notification.
    pub provider: ProviderId,
    /// Provider-defined JSON-RPC notification method.
    pub method: String,
    /// Notification payload.
    pub params: Value,
}

/// An identifier assigned by the host to one outgoing JSON-RPC request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProviderRequestId(pub(crate) u64);

impl ProviderRequestId {
    /// Returns the numeric JSON-RPC request identifier.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Errors returned by provider discovery, transport, or a remote JSON-RPC method.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderError {
    /// No discovered package has the requested provider ID.
    UnknownProvider(String),
    /// Launching a provider subprocess failed.
    Spawn {
        /// Provider that could not be launched.
        provider: String,
        /// Platform error description.
        message: String,
    },
    /// Provider-process I/O failed.
    Transport {
        /// Provider whose transport failed.
        provider: String,
        /// Transport error description.
        message: String,
    },
    /// A provider violated the JSON-RPC contract.
    Protocol {
        /// Provider that sent invalid data.
        provider: String,
        /// Protocol violation description.
        message: String,
    },
    /// A provider returned a JSON-RPC error response.
    Remote {
        /// Provider that returned the error.
        provider: String,
        /// JSON-RPC error code.
        code: i64,
        /// Provider-supplied error message.
        message: String,
        /// Optional provider-supplied structured details.
        data: Option<Value>,
    },
    /// The request was cancelled before its response arrived.
    Cancelled(ProviderRequestId),
    /// A bounded provider request did not complete before its deadline.
    Timeout {
        /// Provider that exceeded the deadline.
        provider: String,
        /// Method whose response did not arrive.
        method: String,
    },
    /// The host has been shut down and accepts no new work.
    Shutdown,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProvider(provider) => write!(formatter, "unknown provider `{provider}`"),
            Self::Spawn { provider, message } => {
                write!(
                    formatter,
                    "could not start provider `{provider}`: {message}"
                )
            }
            Self::Transport { provider, message } => {
                write!(
                    formatter,
                    "provider `{provider}` transport failed: {message}"
                )
            }
            Self::Protocol { provider, message } => {
                write!(
                    formatter,
                    "provider `{provider}` sent invalid protocol data: {message}"
                )
            }
            Self::Remote {
                provider,
                code,
                message,
                ..
            } => write!(
                formatter,
                "provider `{provider}` returned JSON-RPC error {code}: {message}"
            ),
            Self::Cancelled(id) => write!(formatter, "provider request {} was cancelled", id.get()),
            Self::Timeout { provider, method } => {
                write!(
                    formatter,
                    "provider `{provider}` request `{method}` timed out"
                )
            }
            Self::Shutdown => formatter.write_str("provider host has shut down"),
        }
    }
}

impl Error for ProviderError {}
