//! Public projections for hierarchical instruction context.

use crate::{AgentId, AgentRoleSummary, ModelRef};
use serde::{Deserialize, Serialize};

/// The filesystem role of one `AGENTS.md` source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionSourceKind {
    /// User-wide instructions stored below the Misy data directory.
    Global,
    /// Instructions at the detected project root.
    ProjectRoot,
    /// Instructions scoped to a project subdirectory.
    Nested,
}

/// The directory scope controlled by one instruction source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "directory", rename_all = "snake_case")]
pub enum InstructionScope {
    /// Applies regardless of the target filesystem path.
    Global,
    /// Applies throughout the current project.
    ProjectRoot(String),
    /// Applies only at and below the named directory.
    Nested(String),
}

/// Whether an instruction source can contribute content to a request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionSourceStatus {
    /// The complete retained source is active.
    Active,
    /// Only a bounded prefix of the source is active.
    Truncated,
    /// The source could not be loaded safely.
    Blocked,
}

/// Bounded, content-free metadata for one instruction source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstructionSourceSummary {
    /// Source role in the hierarchy.
    pub kind: InstructionSourceKind,
    /// Display-safe absolute source path.
    pub display_path: String,
    /// Filesystem area controlled by the source.
    pub scope: InstructionScope,
    /// Bytes in the original UTF-8 source.
    pub original_bytes: usize,
    /// Bytes retained in the rendered project chain.
    pub retained_bytes: usize,
    /// Whether the rendered source was shortened by a budget.
    pub truncated: bool,
    /// Current source state.
    pub status: InstructionSourceStatus,
}

/// Stable reason why an instruction source needs user attention.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionWarningReason {
    /// The path exists but does not resolve to a regular file.
    NotRegularFile,
    /// The source is not valid UTF-8.
    InvalidUtf8,
    /// The source exceeds its individual hard limit.
    SourceTooLarge,
    /// A root-conversation source or memory limit was reached.
    ConversationLimit,
    /// A filesystem operation failed.
    Io(String),
    /// The effective project chain was shortened to its shared budget.
    Truncated,
}

/// Agent session that owns a warning or blocked source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionOwner {
    /// Root conversation.
    Main,
    /// Independent child conversation.
    Child(AgentId),
}

/// Client-safe diagnostic for an instruction failure or truncation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstructionWarning {
    /// Content-free source metadata.
    pub source: InstructionSourceSummary,
    /// Typed warning reason.
    pub reason: InstructionWarningReason,
    /// Session that observed the condition.
    pub owner: InstructionOwner,
}

/// Which instruction-scope set a context report represents.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextReportState {
    /// Only global and project-root instructions are active.
    Base,
    /// A submission is currently accumulating nested scopes.
    ActiveSubmission,
    /// The report describes the most recently completed submission.
    LastSubmission,
}

/// One category in the estimated next-request context.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategory {
    /// Built-in Misy system guidance.
    MisyPrompt,
    /// Global and project `AGENTS.md` blocks.
    AgentsMd,
    /// Provider-visible local tool schemas.
    ToolDefinitions,
    /// Effective role names/descriptions sent through the spawn tool contract.
    CustomAgents,
    /// Active provider-facing summary produced by the latest compaction.
    CompactionSummary,
    /// Persisted conversation history and pending metadata.
    Messages,
    /// Unused advertised model context.
    FreeSpace,
}

/// Estimated token count for one [`ContextCategory`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextCategoryUsage {
    /// Category represented by this row.
    pub category: ContextCategory,
    /// Conservative text-token estimate.
    pub estimated_tokens: usize,
}

/// Cheap, content-free projection used by `/context`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextReport {
    /// Selected provider-scoped model, when available.
    pub model: Option<ModelRef>,
    /// Cached user-facing model name, when available.
    pub model_display_name: Option<String>,
    /// Provider-advertised window; zero means unknown.
    pub context_window: u32,
    /// Total estimated text tokens excluding images.
    pub estimated_tokens: usize,
    /// Estimated category breakdown in presentation order.
    pub categories: Vec<ContextCategoryUsage>,
    /// Scope set represented by this report.
    pub state: ContextReportState,
    /// Active or last-submission instruction metadata without contents.
    pub sources: Vec<InstructionSourceSummary>,
    /// Active decoded image count.
    pub image_count: usize,
    /// Active decoded image bytes.
    pub image_bytes: usize,
    /// Deduplicated instruction diagnostics.
    pub warnings: Vec<InstructionWarning>,
    /// Effective role metadata without role instruction contents.
    pub roles: Vec<AgentRoleSummary>,
    /// Effective automatic-compaction threshold for the selected model.
    pub auto_compaction_threshold: Option<usize>,
    /// Tokens reserved below that threshold for the next response and tools.
    pub compaction_reserve_tokens: Option<usize>,
}
