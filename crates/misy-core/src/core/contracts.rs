//! Public values and errors exposed by the headless core.

use crate::{
    ActivitySummary, ConfigError, CredentialError, ImageAttachment, InputModality, Message,
    ModelInfo, ModelRef, ProviderDiscoveryError, ProviderDisplayName, ProviderError, ProviderId,
    ToolCall, ToolResult,
};
use serde_json::Value;
use std::{error::Error, fmt};

/// A stable handle for one asynchronous agent submission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubmissionId(pub(super) u64);

impl SubmissionId {
    /// Returns the stable numeric submission identifier.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A canonical history item retained by the Rust core. Provider metadata stays opaque.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    /// User, assistant, or tool message retained for the next provider request.
    pub message: Message,
    /// Images supplied with this message while its submission is active.
    pub attachments: Vec<ImageAttachment>,
    /// Tool calls emitted alongside [`Self::message`].
    pub tool_calls: Vec<ToolCall>,
    /// Local tool results emitted after [`Self::tool_calls`].
    pub tool_results: Vec<ToolResult>,
    /// Opaque metadata returned by the selected provider.
    pub provider_metadata: Value,
}

/// Models discovered from authenticated providers, plus failures isolated to one provider.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AvailableModels {
    /// Models returned by providers that responded successfully.
    pub models: Vec<ModelInfo>,
    /// Failures from individual providers that did not prevent other results.
    pub errors: Vec<ProviderModelError>,
}

/// A provider-specific failure encountered while collecting available models.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderModelError {
    /// Provider whose model listing failed.
    pub provider: ProviderId,
    /// Provider name intended for user-facing error messages.
    pub provider_display_name: ProviderDisplayName,
    /// The provider-listing failure message.
    pub message: String,
}

/// Observable changes emitted by the headless runtime.
// The established public event name is part of the client contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Debug, PartialEq)]
pub enum CoreEvent {
    /// A background activity was added or changed state.
    ActivityChanged {
        /// Latest bounded activity projection.
        activity: ActivitySummary,
    },
    /// A published background activity reached a terminal state.
    ///
    /// The final bounded output travels with the event so clients can commit one complete
    /// transcript item without racing a follow-up output query.
    ActivityFinished {
        /// Final metadata and bounded process output.
        output: crate::ActivityOutput,
    },
    /// A package was discovered during core construction.
    ProviderDiscovered {
        /// Discovered provider identifier.
        provider: ProviderId,
    },
    /// A provider's authentication state changed.
    AuthenticationChanged {
        /// Provider whose state changed.
        provider: ProviderId,
        /// Whether the provider now has valid credentials.
        authenticated: bool,
    },
    /// A provider returned its model catalog.
    ModelsListed {
        /// Provider that returned models.
        provider: ProviderId,
        /// Normalized model metadata.
        models: Vec<ModelInfo>,
    },
    /// The direct-interaction model was changed.
    ModelSelected {
        /// Selected provider-scoped model.
        model: ModelRef,
    },
    /// A submitted message was accepted into the FIFO submission queue.
    ///
    /// Emitted under the queue lock before the submission is enqueued, so subscribers observe
    /// acceptances in queue order and always before any other event for the same submission.
    /// The carried message makes the acceptance self-sufficient: clients render the prompt and
    /// the queue preview from this event instead of reconstructing either from the asynchronous
    /// [`MisyCore::submit`](crate::MisyCore::submit) result, which races with the event stream.
    SubmissionAccepted {
        /// Accepted submission.
        submission: SubmissionId,
        /// Message queued for processing, as retained in session history.
        message: Message,
        /// Number of image attachments, without exposing their payloads to clients.
        attachment_count: usize,
    },
    /// A submitted user message began processing.
    SubmissionStarted {
        /// Submission that began.
        submission: SubmissionId,
        /// Model processing the submission.
        model: ModelRef,
    },
    /// A provider streamed another text fragment.
    TextDelta {
        /// Submission receiving the fragment.
        submission: SubmissionId,
        /// Incremental assistant text.
        delta: String,
        /// Opaque metadata associated with the fragment.
        provider_metadata: Value,
    },
    /// A provider requested a local tool invocation.
    ToolCall {
        /// Submission receiving the request.
        submission: SubmissionId,
        /// Requested local tool invocation.
        call: ToolCall,
        /// Opaque metadata associated with the request.
        provider_metadata: Value,
    },
    /// A local tool invocation completed.
    ToolResult {
        /// Submission that dispatched the tool.
        submission: SubmissionId,
        /// Tool execution outcome.
        result: ToolResult,
    },
    /// A submission completed successfully.
    Completed {
        /// Completed submission.
        submission: SubmissionId,
    },
    /// A submission was cancelled.
    Cancelled {
        /// Cancelled submission.
        submission: SubmissionId,
    },
    /// A submission ended with an error.
    Failed {
        /// Failed submission.
        submission: SubmissionId,
        /// User-facing failure description.
        message: String,
    },
    /// The core shut down and no longer accepts work.
    Shutdown,
}

/// Errors from core configuration, provider operations, and agent lifecycle checks.
// The established public error name is part of the client contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub enum CoreError {
    /// Local configuration access failed.
    Config(ConfigError),
    /// Opaque credential access failed.
    Credentials(CredentialError),
    /// Provider package discovery failed.
    Discovery(ProviderDiscoveryError),
    /// Provider transport or remote operation failed.
    Provider(ProviderError),
    /// A provider returned malformed model metadata.
    InvalidModels(String),
    /// A provider returned a malformed normalized usage report.
    InvalidUsage(String),
    /// The selected model does not accept one requested input modality.
    UnsupportedInput {
        /// Selected provider-scoped model.
        model: ModelRef,
        /// Input modality rejected by the model.
        modality: InputModality,
    },
    /// A submission exceeded the bounded image count.
    TooManyAttachments {
        /// Number of images supplied by the client.
        found: usize,
        /// Maximum images accepted in one submission.
        maximum: usize,
    },
    /// A submission exceeded the aggregate normalized-image budget.
    AttachmentPayloadTooLarge {
        /// Total normalized image bytes supplied by the client.
        found: usize,
        /// Maximum normalized image bytes accepted in one active request.
        maximum: usize,
    },
    /// The private Tokio runtime could not be started or a runtime task could not complete.
    Runtime(String),
    /// A provider does not advertise the requested optional capability revision.
    UnsupportedCapability {
        /// Provider whose manifest was checked.
        provider: ProviderId,
        /// Optional capability identifier.
        capability: String,
        /// Capability revision required by the caller.
        version: u32,
    },
    /// Account-scoped usage was requested without stored provider credentials.
    ProviderNotAuthenticated(ProviderId),
    /// A client requested an authentication method the provider did not declare.
    UnsupportedAuthMethod {
        /// Provider whose manifest was checked.
        provider: ProviderId,
        /// Provider-local authentication method identifier.
        method: String,
    },
    /// The selected model is not advertised by its provider.
    UnknownModel(ModelRef),
    /// No model has been selected for direct interaction.
    NoModelSelected,
    /// No active submission has this identifier.
    UnknownSubmission(SubmissionId),
    /// The core has been shut down.
    Shutdown,
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "configuration error: {error}"),
            Self::Credentials(error) => write!(formatter, "credential error: {error}"),
            Self::Discovery(error) => write!(formatter, "provider discovery error: {error}"),
            Self::Provider(error) => write!(formatter, "provider error: {error}"),
            Self::InvalidModels(message) => write!(formatter, "invalid models response: {message}"),
            Self::InvalidUsage(message) => write!(formatter, "invalid usage response: {message}"),
            Self::UnsupportedInput { model, modality } => write!(
                formatter,
                "model `{}` does not support {modality:?} input",
                model.model.as_str()
            ),
            Self::TooManyAttachments { found, maximum } => write!(
                formatter,
                "submission contains {found} images; maximum is {maximum}"
            ),
            Self::AttachmentPayloadTooLarge { found, maximum } => write!(
                formatter,
                "submission images contain {found} bytes; maximum is {maximum}"
            ),
            Self::Runtime(message) => write!(formatter, "runtime error: {message}"),
            Self::UnsupportedCapability {
                provider,
                capability,
                version,
            } => write!(
                formatter,
                "provider `{}` does not support capability `{capability}` version {version}",
                provider.as_str()
            ),
            Self::ProviderNotAuthenticated(provider) => write!(
                formatter,
                "provider `{}` is not authenticated",
                provider.as_str()
            ),
            Self::UnsupportedAuthMethod { provider, method } => write!(
                formatter,
                "provider `{}` does not support authentication method `{method}`",
                provider.as_str()
            ),
            Self::UnknownModel(model) => {
                write!(formatter, "unknown model `{}`", model.model.as_str())
            }
            Self::NoModelSelected => formatter.write_str("no model is selected"),
            Self::UnknownSubmission(id) => write!(formatter, "unknown submission {}", id.get()),
            Self::Shutdown => formatter.write_str("misy core has shut down"),
        }
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Credentials(error) => Some(error),
            Self::Discovery(error) => Some(error),
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ConfigError> for CoreError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<CredentialError> for CoreError {
    fn from(error: CredentialError) -> Self {
        Self::Credentials(error)
    }
}

impl From<ProviderDiscoveryError> for CoreError {
    fn from(error: ProviderDiscoveryError) -> Self {
        Self::Discovery(error)
    }
}

impl From<ProviderError> for CoreError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(match error {
            ProviderError::Remote {
                provider,
                code,
                message,
                data,
            } => ProviderError::Remote {
                provider,
                code,
                message,
                data: data.map(strip_credentials),
            },
            error => error,
        })
    }
}

pub(super) fn strip_credentials(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            object.remove("credentials");
            for child in object.values_mut() {
                *child = strip_credentials(std::mem::take(child));
            }
        }
        Value::Array(values) => {
            for child in values {
                *child = strip_credentials(std::mem::take(child));
            }
        }
        _ => {}
    }
    value
}
