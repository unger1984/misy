//! Child-agent contracts and process-local lifecycle management.

mod contracts;
mod handlers;
mod profiles;
mod record;
mod registry;
mod spawn;
mod tools;
mod transcript;
mod tree;

pub use contracts::{
    AgentAttempt, AgentId, AgentSummary, AgentTranscript, AgentTranscriptEntry,
    AgentTranscriptEntryKind,
};
pub(super) use registry::{
    AgentRecord, AgentRegistration, AgentRegistry, InboxDecision, MailboxWait,
};
pub(super) use spawn::dispatch_agent_tool;
pub(crate) use tools::{is_agent_tool, normalize_agent_title, tool_definitions};
use transcript::TranscriptBuffer;
