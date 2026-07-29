//! Core-owned background activity contracts shared with frontend clients.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable identifier for one background activity in the current Misy process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ActivityId(u64);

impl ActivityId {
    /// Creates an activity identifier from its process-local sequence number.
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the process-local sequence number.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ActivityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "task-{}", self.0)
    }
}

/// Kind of work represented in the shared activity registry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// A local direct or shell command.
    Task,
    /// A future child-agent session.
    Agent,
}

/// Lifecycle state of one background activity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    /// Accepted but not yet executing.
    Queued,
    /// Currently executing.
    Running,
    /// Paused pending another input or dependency.
    Waiting,
    /// Finished successfully.
    Completed,
    /// Finished unsuccessfully.
    Failed,
    /// Stopped by a user, timeout, or shutdown request.
    Stopped,
}

impl ActivityStatus {
    /// Returns whether no more work can occur for this activity.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Stopped)
    }
}

/// Bounded client projection of one background activity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivitySummary {
    /// Stable process-local identifier.
    pub id: ActivityId,
    /// Activity category used by client filters.
    pub kind: ActivityKind,
    /// Current lifecycle state.
    pub status: ActivityStatus,
    /// Short user-facing label.
    pub title: String,
    /// Command working directory, when applicable.
    pub cwd: Option<String>,
    /// Milliseconds since the Unix epoch when execution began.
    pub started_at_ms: u64,
    /// Process exit code when one is available.
    pub exit_code: Option<i32>,
    /// Whether the command owns a writable pseudo-terminal.
    #[serde(default)]
    pub interactive: bool,
}

/// Stream associated with one ordered activity-output fragment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutputStream {
    /// Bytes captured from standard output, or the unified PTY stream.
    Stdout,
    /// Bytes captured from standard error.
    Stderr,
    /// A core-generated truncation marker between retained output regions.
    System,
}

/// One coalesced fragment in capture-observed output order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivityOutputFragment {
    /// Source stream, or [`ActivityOutputStream::System`] for a truncation marker.
    pub stream: ActivityOutputStream,
    /// Lossy UTF-8 rendering of the captured bytes.
    pub text: String,
}

/// Bounded stdout and stderr snapshot for one command activity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivityOutput {
    /// Activity metadata captured with the output.
    pub activity: ActivitySummary,
    /// Bounded standard output.
    pub stdout: String,
    /// Bounded standard error.
    pub stderr: String,
    /// Whether bytes were omitted from standard output.
    pub stdout_truncated: bool,
    /// Whether bytes were omitted from standard error.
    pub stderr_truncated: bool,
    /// Coalesced fragments in the order observed by the core capture tasks.
    #[serde(default)]
    pub fragments: Vec<ActivityOutputFragment>,
    /// Lifecycle or process failure detail, when present.
    pub message: Option<String>,
}
