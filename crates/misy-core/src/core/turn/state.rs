//! State owned by one independent model-turn session.

use super::super::ActiveSubmission;
use crate::{
    AgentId, HistoryEntry, ModelRef, SubmissionId, TodoItem, core::instructions::InstructionSession,
};
use std::sync::{Arc, Mutex};

/// Identifies the session that owns a model turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentTurnIdentity {
    /// The user-visible root conversation.
    Main,
    /// An isolated child conversation.
    Child(AgentId),
}

/// Controls whether a turn may append entries to the root conversation store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PersistencePolicy {
    /// The main conversation owns append-only session persistence.
    Conversation,
    /// Child conversations are retained only by their agent transcript owner.
    Ephemeral,
}

/// Selects the event projection associated with a turn session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TurnEventSink {
    /// Preserve the established public submission event sequence.
    Submission(SubmissionId),
    /// A future agent manager owns child activity and transcript projection.
    Child,
}

/// All mutable conversation state required by one model-turn loop.
///
/// The main state shares its history with [`MisyCore::history`](crate::MisyCore::history). Child
/// states receive a separately allocated history and therefore cannot mutate or persist root
/// conversation entries.
#[derive(Clone)]
pub(crate) struct AgentTurnState {
    identity: AgentTurnIdentity,
    model: ModelRef,
    history: Arc<Mutex<Vec<HistoryEntry>>>,
    todos: Arc<Mutex<Vec<TodoItem>>>,
    active: Arc<ActiveSubmission>,
    persistence: PersistencePolicy,
    events: TurnEventSink,
    instructions: Arc<Mutex<InstructionSession>>,
}

impl AgentTurnState {
    /// Creates the root session state that preserves existing submission semantics.
    pub(crate) fn main(
        model: ModelRef,
        history: Arc<Mutex<Vec<HistoryEntry>>>,
        todos: Arc<Mutex<Vec<TodoItem>>>,
        active: Arc<ActiveSubmission>,
        submission: SubmissionId,
        instructions: Arc<Mutex<InstructionSession>>,
    ) -> Self {
        Self {
            identity: AgentTurnIdentity::Main,
            model,
            history,
            todos,
            active,
            persistence: PersistencePolicy::Conversation,
            events: TurnEventSink::Submission(submission),
            instructions,
        }
    }

    /// Creates an isolated child state for the agent manager to run later.
    pub(crate) fn child(
        id: AgentId,
        model: ModelRef,
        history: Vec<HistoryEntry>,
        instructions: Arc<Mutex<InstructionSession>>,
    ) -> Self {
        Self {
            identity: AgentTurnIdentity::Child(id),
            model,
            history: Arc::new(Mutex::new(history)),
            todos: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(ActiveSubmission::new()),
            persistence: PersistencePolicy::Ephemeral,
            events: TurnEventSink::Child,
            instructions,
        }
    }

    pub(crate) fn identity(&self) -> AgentTurnIdentity {
        self.identity
    }

    pub(crate) fn model(&self) -> &ModelRef {
        &self.model
    }

    pub(crate) fn history(&self) -> &Arc<Mutex<Vec<HistoryEntry>>> {
        &self.history
    }

    pub(crate) fn todos(&self) -> &Arc<Mutex<Vec<TodoItem>>> {
        &self.todos
    }

    pub(crate) fn active(&self) -> &Arc<ActiveSubmission> {
        &self.active
    }

    pub(crate) fn persistence(&self) -> PersistencePolicy {
        self.persistence
    }

    pub(crate) fn events(&self) -> TurnEventSink {
        self.events
    }

    pub(crate) fn instructions(&self) -> &Arc<Mutex<InstructionSession>> {
        &self.instructions
    }

    pub(crate) fn permits_agent_tools(&self) -> bool {
        self.identity == AgentTurnIdentity::Main
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentTurnState, PersistencePolicy};
    use crate::core::instructions::InstructionSession;
    use crate::{AgentId, ModelId, ModelRef, ProviderId};
    use std::sync::{Arc, Mutex};

    #[test]
    fn child_state_is_ephemeral_and_disallows_agent_tools() {
        let state = AgentTurnState::child(
            AgentId::new(1),
            ModelRef::new(ProviderId::new("test"), ModelId::new("model")),
            Vec::new(),
            Arc::new(Mutex::new(InstructionSession::new(
                crate::core::instructions::InstructionRoot::load(
                    &crate::MisyPaths::from_root(tempfile::tempdir().expect("tempdir").path()),
                    false,
                )
                .expect("instructions"),
                crate::InstructionOwner::Child(AgentId::new(1)),
            ))),
        );

        assert_eq!(state.persistence(), PersistencePolicy::Ephemeral);
        assert!(!state.permits_agent_tools());
    }
}
