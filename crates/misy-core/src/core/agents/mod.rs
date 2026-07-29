//! Child-agent contracts and process-local lifecycle management.

mod contracts;
mod handlers;
mod record;
mod registry;
mod tools;
mod transcript;

pub use contracts::{
    AgentId, AgentSummary, AgentTranscript, AgentTranscriptEntry, AgentTranscriptEntryKind,
};
pub(super) use handlers::dispatch_agent_tool;
pub(super) use registry::{AgentRecord, AgentRegistry, InboxDecision, MailboxWait};
pub(crate) use tools::{is_agent_tool, normalize_agent_title, tool_definitions};
use transcript::TranscriptBuffer;
