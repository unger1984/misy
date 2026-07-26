//! Side-effect-free input vocabulary shared by the terminal and test clients.

use crate::{ModelRef, ProviderId};

/// Intent created from keyboard or slash-command input before side effects run.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    /// No state change.
    Noop,
    /// Open provider settings.
    ShowProviders,
    /// Start provider authorization.
    StartAuth(ProviderId),
    /// Open the model picker.
    ShowModels,
    /// Select a model.
    SelectModel(ModelRef),
    /// Submit a user prompt.
    SubmitPrompt(String),
    /// Append streamed assistant text.
    AppendAssistantText(String),
    /// Append a tool call.
    AppendToolCall {
        /// Provider-scoped tool-call identifier.
        id: String,
        /// Registered tool name.
        name: String,
    },
    /// Append a tool result.
    AppendToolResult {
        /// Tool-call identifier being completed.
        id: String,
        /// Whether tool execution failed.
        is_error: bool,
    },
    /// Recall an older prompt.
    HistoryPrevious,
    /// Recall a newer prompt.
    HistoryNext,
    /// Move list selection upward.
    PickerUp,
    /// Move list selection downward.
    PickerDown,
    /// Accept the selected list row.
    PickerConfirm,
    /// Dismiss the current bottom-pane surface.
    PickerBack,
    /// Cancel active work, shut down providers, and exit.
    CancelAndExit,
}

/// Normalized key handled by deterministic TUI state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiKey {
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Page-up key reserved for transcript navigation.
    PageUp,
    /// Page-down key reserved for transcript navigation.
    PageDown,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Backspace key.
    Backspace,
    /// Forward-delete key.
    Delete,
    /// Enter key used for submission or acceptance.
    Enter,
    /// Tab key used for popup acceptance.
    Tab,
    /// Escape key used for local dismissal.
    Escape,
    /// Explicit composer newline, normally Shift+Enter.
    Newline,
    /// Direct one-based selection from a numbered modal list.
    SelectIndex(usize),
}

/// Active lower-panel surface exposed for deterministic client tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiMode {
    #[default]
    /// Composer focus with no modal view.
    Input,
    /// Provider selection view.
    ProviderList,
    /// Settings for one provider.
    ProviderDetail,
    /// Model selection view.
    ModelList,
}

/// Maps navigation keys to their old action vocabulary for API compatibility.
pub fn map_key(mode: UiMode, key: UiKey) -> UiAction {
    if mode == UiMode::Input {
        return match key {
            UiKey::Up => UiAction::HistoryPrevious,
            UiKey::Down => UiAction::HistoryNext,
            _ => UiAction::Noop,
        };
    }
    match key {
        UiKey::Up => UiAction::PickerUp,
        UiKey::Down => UiAction::PickerDown,
        UiKey::Enter => UiAction::PickerConfirm,
        UiKey::Escape => UiAction::PickerBack,
        UiKey::SelectIndex(_) => UiAction::Noop,
        _ => UiAction::Noop,
    }
}

/// Converts submitted text into a command or prompt action.
///
/// # Errors
///
/// Returns an explanatory error for unknown commands and obsolete command arguments.
pub fn map_input(input: &str) -> Result<UiAction, String> {
    if input.trim().is_empty() {
        return Ok(UiAction::Noop);
    }
    if !input.starts_with('/') {
        return Ok(UiAction::SubmitPrompt(input.to_owned()));
    }
    match input.trim() {
        "/provider" => Ok(UiAction::ShowProviders),
        "/model" => Ok(UiAction::ShowModels),
        command if command.starts_with("/provider ") => {
            Err("use `/provider` and choose from the picker".to_owned())
        }
        command if command.starts_with("/model ") => {
            Err("use `/model` and choose from the picker".to_owned())
        }
        command => Err(format!("unknown command `{command}`")),
    }
}
