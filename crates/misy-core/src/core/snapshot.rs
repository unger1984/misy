//! Cheap, in-memory state snapshots for headless-core clients.

use super::{MisyCore, SubmissionId};
use crate::{ModelRef, ProviderId};

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
        let auth_states = state
            .auth_states
            .lock()
            .expect("authentication state mutex must not be poisoned");
        let (active_submission, queued_submissions) = submission_queue.snapshot();
        CoreSnapshot {
            selected_model: selected_model.clone(),
            active_submission,
            queued_submissions,
            providers: auth_states.values().cloned().collect(),
        }
    }
}
