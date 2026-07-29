//! Cheap, in-memory state snapshots for headless-core clients.

use super::{MisyCore, SubmissionId};
use crate::{ActivitySummary, AgentSummary, ModelRef, ProviderId};

const MAX_TERMINAL_ACTIVITIES: usize = 20;

/// One provider's cached local authentication state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAuthState {
    /// Stable identifier of the discovered provider.
    pub id: ProviderId,
    /// Whether the core currently considers the provider authenticated.
    pub authenticated: bool,
    /// Provider-local credential method, when stored credentials declared one.
    pub credential_method: Option<String>,
}

/// A point-in-time projection of client-visible core state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreSnapshot {
    /// Active activities followed by the most recent terminal activities.
    pub activities: Vec<ActivitySummary>,
    /// Live child agents followed by retained terminal child agents.
    pub agents: Vec<AgentSummary>,
    /// Model selected for direct interaction, if any.
    pub selected_model: Option<ModelRef>,
    /// Submission currently occupying the FIFO session slot, if any.
    ///
    /// A terminal submission event is published only after this field no longer contains that
    /// submission.
    pub active_submission: Option<SubmissionId>,
    /// Accepted submissions still waiting for the FIFO session slot.
    pub queued_submissions: Vec<SubmissionId>,
    /// Cached authentication state for every discovered provider, ordered by provider ID.
    pub providers: Vec<ProviderAuthState>,
}

impl MisyCore {
    /// Returns a cheap snapshot of core-owned state without filesystem, process, or network I/O.
    ///
    /// A cancellation request disappears from this projection immediately, even while its worker
    /// finishes provider cleanup. This lets clients stop presenting work that can no longer run.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task panicked while holding one of the short-lived state mutexes.
    pub fn snapshot(&self) -> CoreSnapshot {
        let state = &self.inner.state;
        // Acquiring these locks in this order keeps the multi-field projection coherent. No
        // mutator holds more than one of them, and none is held across I/O or an await point.
        let selected_model = state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned");
        let submission_queue = state
            .submission_queue
            .lock()
            .expect("submission queue mutex must not be poisoned");
        let credential_states = state
            .credential_states
            .lock()
            .expect("credential state mutex must not be poisoned");
        let (active_submission, queued_submissions) = submission_queue.snapshot();
        let agents = state.agents.list();
        let pinned = state.agents.pinned_activity_ids();
        let mut activities = state.dispatcher.activities();
        activities.extend(agents.iter().map(AgentSummary::activity_summary));
        activities.sort_by_key(|activity| {
            (
                activity.status.is_terminal(),
                activity.status.is_terminal() && !pinned.contains(&activity.id),
                std::cmp::Reverse(activity.started_at_ms),
            )
        });
        let mut terminal_count = 0;
        activities.retain(|activity| {
            if !activity.status.is_terminal() {
                return true;
            }
            terminal_count += 1;
            terminal_count <= MAX_TERMINAL_ACTIVITIES
        });
        CoreSnapshot {
            activities,
            agents,
            selected_model: selected_model.clone(),
            active_submission,
            queued_submissions,
            providers: credential_states
                .values()
                .map(|state| state.auth.clone())
                .collect(),
        }
    }
}
