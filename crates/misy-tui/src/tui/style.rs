//! Shared visual language for the terminal client.

use ratatui::style::{Color, Modifier, Style};

/// Returns the low-contrast style used for supporting information.
pub(super) fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Returns the accent style for active controls and transcript markers.
pub(super) fn accent() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// Returns the full-row background used for submitted prompts.
pub(super) fn user_message() -> Style {
    Style::default().bg(Color::DarkGray)
}

/// Returns the full-row highlight for the active popup or list item.
pub(super) fn selected() -> Style {
    Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Returns the blue tab background used only by the model picker.
pub(super) fn model_tab_active() -> Style {
    Style::default()
        .bg(Color::Blue)
        .add_modifier(Modifier::BOLD)
}

/// Returns a subdued span style that remains readable on an active row.
pub(super) fn selected_muted() -> Style {
    muted().bg(Color::DarkGray)
}

/// Returns the style used for successful provider status markers.
pub(super) fn success() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

/// Returns a successful marker style that preserves a selected-row background.
pub(super) fn selected_success() -> Style {
    success().bg(Color::DarkGray)
}

/// Returns the style used for failures in the transcript and tool output.
pub(super) fn error() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}
