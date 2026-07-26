use crate::ProviderId;
use serde_json::Value;
use std::{error::Error, fmt};

/// The JSON-RPC protocol revision implemented by this host.
pub const PROVIDER_PROTOCOL_VERSION: u32 = 1;

/// Methods every v1 provider package must implement.
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
            Self::Shutdown => formatter.write_str("provider host has shut down"),
        }
    }
}

impl Error for ProviderError {}
