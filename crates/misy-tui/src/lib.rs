//! Deterministic terminal client for the headless Misy core.
//!
//! [`run`] drives the terminal session over a [`MisyCore`](misy_core::MisyCore) instance. The
//! module tree stays crate-private so the crate contract is exactly the set of names re-exported
//! here; integration tests under `tests/` consume only this surface.

mod tui;

pub use tui::{
    BrowserHandoff, BrowserPlatform, SessionStart, TranscriptRow, TuiClient, UiAction, UiKey,
    UiMode, UiState, browser_command, map_input, map_key, render, run, validate_authorization_url,
};
