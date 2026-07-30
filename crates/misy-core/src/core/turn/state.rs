//! State owned by one independent model-turn session.

use super::super::ActiveSubmission;
use crate::{
    AgentId, HistoryEntry, ModelRef, SubmissionId, TodoItem, core::instructions::InstructionSession,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

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
    thinking: Option<String>,
    role: Option<String>,
    role_instructions: Option<String>,
    role_catalog: String,
    system_override: Option<String>,
    max_output_tokens: Option<u32>,
    allowed_tools: Option<BTreeSet<String>>,
    history: Arc<Mutex<Vec<HistoryEntry>>>,
    todos: Arc<Mutex<Vec<TodoItem>>>,
    active: Arc<ActiveSubmission>,
    fallback_closed: Arc<AtomicBool>,
    persistence: PersistencePolicy,
    events: TurnEventSink,
    instructions: Arc<Mutex<InstructionSession>>,
}

pub(crate) struct ChildTurnConfig {
    pub(crate) role: Option<String>,
    pub(crate) role_instructions: Option<String>,
    pub(crate) allowed_tools: Option<BTreeSet<String>>,
    pub(crate) history: Vec<HistoryEntry>,
    pub(crate) instructions: Arc<Mutex<InstructionSession>>,
    pub(crate) role_catalog: String,
}

impl AgentTurnState {
    /// Creates the root session state that preserves existing submission semantics.
    pub(crate) fn main(
        model: ModelRef,
        thinking: Option<String>,
        history: Arc<Mutex<Vec<HistoryEntry>>>,
        todos: Arc<Mutex<Vec<TodoItem>>>,
        active: Arc<ActiveSubmission>,
        submission: SubmissionId,
        instructions: Arc<Mutex<InstructionSession>>,
    ) -> Self {
        Self {
            identity: AgentTurnIdentity::Main,
            model,
            thinking,
            role: None,
            role_instructions: None,
            role_catalog: String::new(),
            system_override: None,
            max_output_tokens: None,
            allowed_tools: None,
            history,
            todos,
            active,
            fallback_closed: Arc::new(AtomicBool::new(false)),
            persistence: PersistencePolicy::Conversation,
            events: TurnEventSink::Submission(submission),
            instructions,
        }
    }

    /// Creates an isolated child state for the agent manager to run later.
    pub(crate) fn child(
        id: AgentId,
        profile: crate::ModelProfile,
        config: ChildTurnConfig,
    ) -> Self {
        Self {
            identity: AgentTurnIdentity::Child(id),
            model: profile.model,
            thinking: profile.thinking,
            role: config.role,
            role_instructions: config.role_instructions,
            role_catalog: config.role_catalog,
            system_override: None,
            max_output_tokens: None,
            allowed_tools: config.allowed_tools,
            history: Arc::new(Mutex::new(config.history)),
            todos: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(ActiveSubmission::new()),
            fallback_closed: Arc::new(AtomicBool::new(false)),
            persistence: PersistencePolicy::Ephemeral,
            events: TurnEventSink::Child,
            instructions: config.instructions,
        }
    }

    pub(crate) fn identity(&self) -> AgentTurnIdentity {
        self.identity
    }

    pub(crate) fn model(&self) -> &ModelRef {
        &self.model
    }

    pub(crate) fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
    }

    pub(crate) fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    pub(crate) fn with_profile(&self, profile: &crate::ModelProfile) -> Self {
        Self {
            identity: self.identity,
            model: profile.model.clone(),
            thinking: profile.thinking.clone(),
            role: self.role.clone(),
            role_instructions: self.role_instructions.clone(),
            role_catalog: self.role_catalog.clone(),
            system_override: self.system_override.clone(),
            max_output_tokens: self.max_output_tokens,
            allowed_tools: self.allowed_tools.clone(),
            history: Arc::clone(&self.history),
            todos: Arc::clone(&self.todos),
            active: Arc::clone(&self.active),
            fallback_closed: Arc::clone(&self.fallback_closed),
            persistence: self.persistence,
            events: self.events,
            instructions: Arc::clone(&self.instructions),
        }
    }

    pub(crate) fn role_instructions(&self) -> Option<&str> {
        self.role_instructions.as_deref()
    }

    pub(crate) fn role_catalog(&self) -> &str {
        &self.role_catalog
    }

    pub(crate) fn with_role_catalog(mut self, role_catalog: String) -> Self {
        self.role_catalog = role_catalog;
        self
    }

    pub(crate) fn system_override(&self) -> Option<&str> {
        self.system_override.as_deref()
    }

    pub(crate) fn compaction(
        profile: crate::ModelProfile,
        history: Vec<HistoryEntry>,
        instructions: Arc<Mutex<InstructionSession>>,
        system_prompt: String,
        active: Arc<ActiveSubmission>,
        max_output_tokens: u32,
    ) -> Self {
        Self {
            identity: AgentTurnIdentity::Main,
            model: profile.model,
            thinking: profile.thinking,
            role: None,
            role_instructions: None,
            role_catalog: String::new(),
            system_override: Some(system_prompt),
            max_output_tokens: Some(max_output_tokens),
            allowed_tools: Some(BTreeSet::new()),
            history: Arc::new(Mutex::new(history)),
            todos: Arc::new(Mutex::new(Vec::new())),
            active,
            fallback_closed: Arc::new(AtomicBool::new(false)),
            persistence: PersistencePolicy::Ephemeral,
            events: TurnEventSink::Child,
            instructions,
        }
    }

    pub(crate) fn allowed_tools(&self) -> Option<&BTreeSet<String>> {
        self.allowed_tools.as_ref()
    }

    pub(crate) fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
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

    pub(crate) fn close_fallback(&self) {
        self.fallback_closed.store(true, Ordering::Release);
    }

    pub(crate) fn fallback_closed(&self) -> bool {
        self.fallback_closed.load(Ordering::Acquire)
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
}

#[cfg(test)]
mod tests {
    use super::{AgentTurnState, PersistencePolicy};
    use crate::core::instructions::InstructionSession;
    use crate::{AgentId, ModelId, ModelRef, ProviderId};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    #[test]
    fn child_state_is_ephemeral_and_honors_empty_allowlist() {
        let state = AgentTurnState::child(
            AgentId::new(1),
            crate::ModelProfile::new(
                ModelRef::new(ProviderId::new("test"), ModelId::new("model")),
                None,
            ),
            super::ChildTurnConfig {
                role: None,
                role_instructions: None,
                allowed_tools: Some(BTreeSet::new()),
                history: Vec::new(),
                instructions: Arc::new(Mutex::new(InstructionSession::new(
                    crate::core::instructions::InstructionRoot::load(
                        &crate::MisyPaths::from_root(tempfile::tempdir().expect("tempdir").path()),
                        false,
                    )
                    .expect("instructions"),
                    crate::InstructionOwner::Child(AgentId::new(1)),
                ))),
                role_catalog: String::new(),
            },
        );

        assert_eq!(state.persistence(), PersistencePolicy::Ephemeral);
        assert!(state.allowed_tools().is_some_and(BTreeSet::is_empty));
    }

    #[test]
    fn profile_changes_preserve_the_fallback_boundary() {
        let state = AgentTurnState::child(
            AgentId::new(1),
            crate::ModelProfile::new(
                ModelRef::new(ProviderId::new("test"), ModelId::new("first")),
                None,
            ),
            super::ChildTurnConfig {
                role: None,
                role_instructions: None,
                allowed_tools: None,
                history: Vec::new(),
                instructions: Arc::new(Mutex::new(InstructionSession::new(
                    crate::core::instructions::InstructionRoot::load(
                        &crate::MisyPaths::from_root(tempfile::tempdir().expect("tempdir").path()),
                        false,
                    )
                    .expect("instructions"),
                    crate::InstructionOwner::Child(AgentId::new(1)),
                ))),
                role_catalog: String::new(),
            },
        );
        state.close_fallback();
        let switched = state.with_profile(&crate::ModelProfile::new(
            ModelRef::new(ProviderId::new("test"), ModelId::new("second")),
            None,
        ));
        assert!(switched.fallback_closed());
    }
}
