//! Deterministic terminal client for the headless Misy core.

mod action;
mod activity_picker;
mod auth_flow;
mod auth_prompt;
mod bottom_surface;
mod browser;
mod client;
mod clipboard;
mod composer;
mod composer_attachment;
mod context_popup;
mod context_view;
mod display_width;
mod history;
mod keymap;
mod list;
mod model_picker;
mod model_popup;
mod model_refresh;
mod model_view;
mod popup;
mod presentation;
mod question_dialog;
mod render;
mod screen_selection;
mod session_picker;
mod startup_header;
mod state;
mod style;
mod terminal;
mod tool_render;
mod transcript_render;
mod usage;

pub use action::{UiAction, UiKey, UiMode, map_input, map_key};
pub use browser::{BrowserHandoff, BrowserPlatform, browser_command, validate_authorization_url};
// These stable public names are intentionally prefixed with `Tui` because callers import them
// through the crate facade as well as this compatibility module.
#[allow(clippy::module_name_repetitions)]
pub use client::TuiClient;
pub use render::render;
pub use state::{TranscriptRow, UiState};
pub use terminal::{SessionStart, run};
