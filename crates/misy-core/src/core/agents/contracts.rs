//! Public projections for independent child-agent sessions.

use crate::{
    ActivityId, ActivityKind, ActivityStatus, ActivitySummary, InstructionSourceSummary, ModelRef,
    ToolCall, ToolResult,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable process-local identifier for one child-agent session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AgentId(u64);

impl AgentId {
    /// Creates an identifier from its process-local sequence number.
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the process-local sequence number.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "agent-{}", self.0)
    }
}

/// Bounded roster projection for one live or recently completed child agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSummary {
    /// Stable process-local child identifier.
    pub id: AgentId,
    /// Shared activity identifier used by generic client actions.
    pub activity_id: ActivityId,
    /// Presentation label derived from the assignment or explicit description.
    pub title: String,
    /// Provider-scoped model used by the child.
    pub model: ModelRef,
    /// Current child lifecycle state.
    pub status: ActivityStatus,
    /// Whether the spawning tool call returned before the child completed.
    pub run_in_background: bool,
    /// Milliseconds since the Unix epoch when the child was registered.
    pub started_at_ms: u64,
    /// Milliseconds since the Unix epoch when the child became terminal.
    pub finished_at_ms: Option<u64>,
    /// Bounded terminal result or failure description.
    pub terminal_message: Option<String>,
}

impl AgentSummary {
    pub(crate) fn activity_summary(&self) -> ActivitySummary {
        ActivitySummary {
            id: self.activity_id,
            kind: ActivityKind::Agent,
            agent_id: Some(self.id),
            status: self.status,
            title: self.title.clone(),
            cwd: None,
            started_at_ms: self.started_at_ms,
            exit_code: None,
            interactive: false,
        }
    }
}

/// Semantic category for one entry in a client-visible child transcript.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTranscriptEntryKind {
    /// Initial assignment supplied by the parent.
    Assignment,
    /// Follow-up message supplied through `agent_message`.
    UserMessage,
    /// Assistant text emitted by the child.
    Assistant,
    /// Local tool request emitted by the child.
    ToolCall,
    /// Local tool execution outcome.
    ToolResult,
    /// Final child status and result.
    Terminal,
}

/// One bounded semantic item in a child-agent transcript.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentTranscriptEntry {
    /// Entry category used by clients for rendering.
    pub kind: AgentTranscriptEntryKind,
    /// Display-safe text associated with the entry.
    pub content: String,
    /// Tool request when this entry represents a tool call.
    pub tool_call: Option<ToolCall>,
    /// Tool outcome when this entry represents a tool result.
    pub tool_result: Option<ToolResult>,
    /// Count of image attachments without copying their payloads to clients.
    #[serde(default)]
    pub attachment_count: usize,
}

/// Bounded transcript projection for one child-agent session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentTranscript {
    /// Current roster metadata for the child.
    pub agent: AgentSummary,
    /// Semantic entries retained within the shared activity-output budget.
    pub entries: Vec<AgentTranscriptEntry>,
    /// Whether older transcript content was omitted to stay within the budget.
    pub truncated: bool,
    /// Content-free instruction sources used by the child before it terminated.
    pub instruction_sources: Vec<InstructionSourceSummary>,
}
