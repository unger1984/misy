//! Session-scoped model-turn state, history projections, and shared loop machinery.

mod fork;
mod r#loop;
mod state;

pub(crate) use fork::{child_history, forkable_prefix};
pub(crate) use r#loop::{MAX_MODEL_TURNS, run_turns, run_turns_with_limit};
pub(crate) use state::{AgentTurnIdentity, AgentTurnState, PersistencePolicy, TurnEventSink};
