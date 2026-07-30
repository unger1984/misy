//! Transcript row model and its projection accessors.
//!
//! The transcript is the TUI-owned projection of the core event stream; this
//! module holds the row type rendered by the terminal and the append/get
//! surface shared by the event projection ([`super::events`]) and the client.

use super::UiState;
use misy_core::{ActivityOutput, CompactionCheckpoint, HistoryEntry, MessageRole};
use std::{fmt, time::Duration};

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
    /// Successful completion of a turn that performed tool work.
    WorkSeparator {
        /// Duration captured when the core advanced its active submission.
        elapsed: Duration,
    },
    /// Persisted service divider for one successful active-history compaction.
    Compaction(CompactionCheckpoint),
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

    /// Returns whether tool output uses the expanded viewport budget.
    pub fn tool_output_expanded(&self) -> bool {
        self.tool_output_expanded
    }

    pub(in crate::tui) fn toggle_tool_output(&mut self) {
        self.tool_output_expanded = !self.tool_output_expanded;
    }

    pub(in crate::tui) fn add_error(&mut self, error: impl fmt::Display) {
        self.transcript
            .push(TranscriptRow::Error(error.to_string()));
    }

    pub(in crate::tui) fn add_info(&mut self, message: impl Into<String>) {
        self.transcript.push(TranscriptRow::Info(message.into()));
    }

    pub(in crate::tui) fn clear_conversation(&mut self) {
        self.transcript.clear();
        self.prompt_text.clear();
        self.response_submission = None;
        self.cancelled_submissions.clear();
        self.submission_started_at = None;
        self.turn_had_tool_activity = false;
        self.terminal_turn = None;
        self.response_started = false;
        self.view = None;
    }

    pub(in crate::tui) fn replay_history(
        &mut self,
        history: &[HistoryEntry],
        compactions: &[CompactionCheckpoint],
    ) {
        self.clear_conversation();
        for (index, entry) in history.iter().enumerate() {
            self.replay_compactions(compactions, index);
            self.replay_entry(entry);
        }
        self.replay_compactions(compactions, history.len());
        self.response_submission = None;
    }

    fn replay_compactions(&mut self, compactions: &[CompactionCheckpoint], history_index: usize) {
        self.transcript.extend(
            compactions
                .iter()
                .filter(|checkpoint| checkpoint.history_len == history_index)
                .cloned()
                .map(TranscriptRow::Compaction),
        );
    }

    pub(in crate::tui) fn add_compaction(&mut self, checkpoint: CompactionCheckpoint) {
        self.transcript.push(TranscriptRow::Compaction(checkpoint));
    }

    fn replay_entry(&mut self, entry: &HistoryEntry) {
        match entry.message.role {
            MessageRole::User => {
                let mut text = (1..=entry.attachments.len())
                    .map(|index| format!("[Image #{index}]"))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !entry.message.content.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&entry.message.content);
                }
                self.transcript.push(TranscriptRow::UserPrompt(text));
            }
            MessageRole::Assistant if !entry.message.content.is_empty() => {
                self.append_assistant_text(None, entry.message.content.clone());
            }
            MessageRole::System | MessageRole::Assistant | MessageRole::Tool => {}
        }
        for call in &entry.tool_calls {
            self.transcript.push(TranscriptRow::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments).ok(),
            });
        }
        for result in &entry.tool_results {
            self.transcript.push(TranscriptRow::ToolResult {
                id: result.tool_call_id.clone(),
                is_error: result.is_error,
                content: Some(result.content.clone()),
            });
        }
    }
}
