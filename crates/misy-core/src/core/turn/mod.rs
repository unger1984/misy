//! Session-scoped model-turn state, history projections, and shared loop machinery.

mod fork;
mod r#loop;
mod preflight;
mod state;
mod stream;

pub(crate) use fork::{child_history, forkable_prefix};
pub(in crate::core) use r#loop::serialize_history_entry;
pub(crate) use r#loop::{MAX_MODEL_TURNS, TurnFailure, run_turns, run_turns_with_limit};
pub(crate) use state::{
    AgentTurnIdentity, AgentTurnState, ChildTurnConfig, PersistencePolicy, TurnEventSink,
};
