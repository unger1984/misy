//! Transcript row model and its projection accessors.
//!
//! The transcript is the TUI-owned projection of the core event stream; this
//! module holds the row type rendered by the terminal and the append/get
//! surface shared by the event projection ([`super::events`]) and the client.

use super::UiState;
use misy_core::ActivityOutput;
use std::fmt;

/// A renderable, user-visible transcript item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptRow {
    /// Provider discovery/status information retained for compatible clients.
    Provider {
        /// Stable provider identifier.
        id: String,
        /// Whether local credentials exist.
        authenticated: bool,
    },
    /// Model discovery information retained for compatible clients.
    Model {
        /// Stable provider identifier.
        provider: String,
        /// Provider-scoped model identifier.
        id: String,
        /// Whether this model is currently selected.
        selected: bool,
    },
    /// Submitted user prompt.
    UserPrompt(String),
    /// One streamed assistant text delta.
    AssistantText(String),
    /// Tool execution start.
    ToolCall {
        /// Provider tool-call identifier.
        id: String,
        /// Registered tool name.
        name: String,
        /// Compact JSON arguments supplied by the provider, when available.
        arguments: Option<String>,
    },
    /// Tool execution completion.
    ToolResult {
        /// Provider tool-call identifier.
        id: String,
        /// Whether the tool failed.
        is_error: bool,
        /// The user-visible local tool result, when available.
        content: Option<String>,
    },
    /// Final output delivered when a background command exits.
    ActivityFinished(ActivityOutput),
    /// Informational lifecycle message.
    Info(String),
    /// User-visible failure.
    Error(String),
}

impl UiState {
    /// Returns the transcript rows in display order.
    pub fn transcript(&self) -> &[TranscriptRow] {
        &self.transcript
    }

    pub(in crate::tui) fn add_error(&mut self, error: impl fmt::Display) {
        self.transcript
            .push(TranscriptRow::Error(error.to_string()));
    }

    pub(in crate::tui) fn add_info(&mut self, message: impl Into<String>) {
        self.transcript.push(TranscriptRow::Info(message.into()));
    }
}
