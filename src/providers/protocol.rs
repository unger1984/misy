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
    pub provider: ProviderId,
    pub method: String,
    pub params: Value,
}

/// An identifier assigned by the host to one outgoing JSON-RPC request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProviderRequestId(pub(crate) u64);

impl ProviderRequestId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Errors returned by provider discovery, transport, or a remote JSON-RPC method.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderError {
    UnknownProvider(String),
    Spawn {
        provider: String,
        message: String,
    },
    Transport {
        provider: String,
        message: String,
    },
    Protocol {
        provider: String,
        message: String,
    },
    Remote {
        provider: String,
        code: i64,
        message: String,
        data: Option<Value>,
    },
    Cancelled(ProviderRequestId),
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
