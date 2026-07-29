//! Side-effect-free input vocabulary shared by the terminal and test clients.

use misy_core::{ModelRef, ProviderId};

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
    /// Open the shared tasks and agents picker.
    ShowActivities,
    /// Open the saved-session picker.
    ShowSessions,
    /// Start a clean conversation while retaining the current saved session.
    NewSession,
    /// Resume a saved conversation by id or unambiguous prefix.
    ResumeSession(String),
    /// Fetch account-limit usage for the selected model's provider.
    ShowUsage,
    /// Open the core-owned context usage report.
    ShowContext,
    /// Select a model.
    SelectModel(ModelRef),
    /// Submit a user prompt.
    SubmitPrompt(String),
    /// Recall an older prompt.
    HistoryPrevious,
    /// Recall a newer prompt.
    HistoryNext,
    /// Move list selection upward.
    PickerUp,
    /// Move list selection downward.
    PickerDown,
    /// Move the model picker to its previous provider tab.
    PickerTabLeft,
    /// Move the model picker to its next provider tab.
    PickerTabRight,
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
    /// Tab key used for popup completion.
    Tab,
    /// Escape key used for local dismissal.
    Escape,
    /// Explicit composer newline, normally Shift+Enter.
    Newline,
    /// Open the shared tasks and agents picker.
    OpenActivities,
    /// Stop the selected activity immediately.
    StopActivity,
    /// Toggle the viewport budget for all transcript tool output.
    ToggleToolOutput,
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
    /// Shared tasks and agents picker.
    ActivityList,
    /// Saved conversation-session picker.
    SessionList,
    /// Confirmation before discarding retained child-agent state.
    Confirmation,
    /// Output for one command activity.
    ActivityDetail,
    /// Structured question dialog owned by the core lifecycle.
    Question,
    /// Scrollable context-usage report.
    Context,
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
        UiKey::Left
            if matches!(
                mode,
                UiMode::ModelList | UiMode::ActivityList | UiMode::Question
            ) =>
        {
            UiAction::PickerTabLeft
        }
        UiKey::Right
            if matches!(
                mode,
                UiMode::ModelList | UiMode::ActivityList | UiMode::Question
            ) =>
        {
            UiAction::PickerTabRight
        }
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
        "/tasks" => Ok(UiAction::ShowActivities),
        "/context" => Ok(UiAction::ShowContext),
        "/new" | "/clear" => Ok(UiAction::NewSession),
        "/resume" => Ok(UiAction::ShowSessions),
        "/status" | "/usage" => Ok(UiAction::ShowUsage),
        "/exit" => Ok(UiAction::CancelAndExit),
        command if command.starts_with("/provider ") => {
            Err("use `/provider` and choose from the picker".to_owned())
        }
        command if command.starts_with("/model ") => {
            Err("use `/model` and choose from the picker".to_owned())
        }
        command if command.starts_with("/tasks ") => {
            Err("use `/tasks` without arguments".to_owned())
        }
        command if command.starts_with("/context ") => {
            Err("use `/context` without arguments".to_owned())
        }
        command if command.starts_with("/new ") => Err("use `/new` without arguments".to_owned()),
        command if command.starts_with("/clear ") => {
            Err("use `/clear` without arguments".to_owned())
        }
        command if command.starts_with("/resume ") => {
            let id = command.strip_prefix("/resume ").unwrap_or_default().trim();
            if id.is_empty() {
                Ok(UiAction::ShowSessions)
            } else {
                Ok(UiAction::ResumeSession(id.to_owned()))
            }
        }
        command if command.starts_with("/usage ") => {
            Err("use `/usage` without arguments".to_owned())
        }
        command if command.starts_with("/status ") => {
            Err("use `/status` without arguments".to_owned())
        }
        command if command.starts_with("/exit ") => Err("use `/exit` without arguments".to_owned()),
        command => Err(format!("unknown command `{command}`")),
    }
}
