//! Deterministic TUI state and the terminal adapter for the headless core.

use crate::{
    CoreError, CoreEvent, Message, MisyCore, ModelInfo, ModelRef, ProviderId, SubmissionId,
    ToolResult,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{self, Stdout},
    process::Command,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Duration,
};

/// Opens an authorization URL outside the terminal UI.
pub trait BrowserHandoff {
    fn open(&mut self, url: &str) -> Result<(), String>;
}

/// Browser handoff using the host platform's standard URL opener.
#[derive(Default)]
pub struct SystemBrowser;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserPlatform {
    MacOs,
    Windows,
    Unix,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub fn browser_command(platform: BrowserPlatform, url: &str) -> Result<BrowserCommand, String> {
    validate_authorization_url(url)?;
    let (program, arguments): (&str, &[&str]) = match platform {
        BrowserPlatform::MacOs => ("open", &[url]),
        BrowserPlatform::Windows => ("rundll32", &["url.dll,FileProtocolHandler", url]),
        BrowserPlatform::Unix => ("xdg-open", &[url]),
    };
    Ok(BrowserCommand {
        program: program.to_owned(),
        args: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
    })
}

impl BrowserHandoff for SystemBrowser {
    fn open(&mut self, url: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        let platform = BrowserPlatform::MacOs;
        #[cfg(target_os = "windows")]
        let platform = BrowserPlatform::Windows;
        #[cfg(all(unix, not(target_os = "macos")))]
        let platform = BrowserPlatform::Unix;
        let specification = browser_command(platform, url)?;

        Command::new(specification.program)
            .args(specification.args)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("could not open authorization URL: {error}"))
    }
}

/// Rejects non-web or control-character authorization locations before the
/// operating-system URL handoff. Windows receives the URL as one process
/// argument and never invokes a command shell.
pub fn validate_authorization_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(char::is_control)
        || url.chars().any(char::is_whitespace)
    {
        return Err("authorization URL must be an http(s) URL without whitespace".to_owned());
    }
    Ok(())
}

/// A renderable, user-visible transcript item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptRow {
    Provider {
        id: String,
        authenticated: bool,
    },
    Model {
        provider: String,
        id: String,
        selected: bool,
    },
    UserPrompt(String),
    AssistantText(String),
    ToolCall {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        is_error: bool,
    },
    Info(String),
    Error(String),
}

impl TranscriptRow {
    fn display(&self) -> String {
        match self {
            Self::Provider { id, authenticated } => {
                format!("provider {id} {}", if *authenticated { "✓" } else { "○" })
            }
            Self::Model {
                provider,
                id,
                selected,
            } => format!("model {provider}/{id}{}", if *selected { " ✓" } else { "" }),
            Self::UserPrompt(prompt) => format!("> {prompt}"),
            Self::AssistantText(text) => text.clone(),
            Self::ToolCall { id, name } => format!("tool {name} ({id}) started"),
            Self::ToolResult { id, is_error } => format!(
                "tool ({id}) {}",
                if *is_error { "failed" } else { "completed" }
            ),
            Self::Info(message) => message.clone(),
            Self::Error(message) => format!("error: {message}"),
        }
    }
}

/// Intentions created from keyboard or slash-command input before side effects run.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    Noop,
    ShowProviders,
    StartAuth(ProviderId),
    ShowModels,
    SelectModel(ModelRef),
    SubmitPrompt(String),
    AppendAssistantText(String),
    AppendToolCall { id: String, name: String },
    AppendToolResult { id: String, is_error: bool },
    ScrollUp,
    ScrollDown,
    HistoryPrevious,
    HistoryNext,
    PickerUp,
    PickerDown,
    PickerConfirm,
    PickerBack,
    CancelAndExit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Escape,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiMode {
    #[default]
    Input,
    ProviderList,
    ProviderDetail,
    ModelList,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProviderOperationKind {
    Status,
    Start,
    Complete,
}

impl ProviderOperationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Status => "Checking status…",
            Self::Start => "Starting authorization…",
            Self::Complete => "Waiting for browser…",
        }
    }
}

enum ProviderOperationResult {
    Status(ProviderId, Result<bool, String>),
    Start(ProviderId, Result<Value, String>),
    Complete(ProviderId, Result<ModelRef, String>),
}

pub fn map_key(mode: UiMode, key: UiKey) -> UiAction {
    if mode == UiMode::Input {
        return match key {
            UiKey::Up => UiAction::HistoryPrevious,
            UiKey::Down => UiAction::HistoryNext,
            UiKey::PageUp => UiAction::ScrollUp,
            UiKey::PageDown => UiAction::ScrollDown,
            UiKey::Enter | UiKey::Escape => UiAction::Noop,
        };
    }
    match key {
        UiKey::Up => UiAction::PickerUp,
        UiKey::Down => UiAction::PickerDown,
        UiKey::PageUp | UiKey::PageDown => UiAction::Noop,
        UiKey::Enter => UiAction::PickerConfirm,
        UiKey::Escape => UiAction::PickerBack,
    }
}

/// Converts one submitted input line into an explicit UI action.
pub fn map_input(input: &str) -> Result<UiAction, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(UiAction::Noop);
    }
    if !input.starts_with('/') {
        return Ok(UiAction::SubmitPrompt(input.to_owned()));
    }
    if input == "/provider" {
        return Ok(UiAction::ShowProviders);
    }
    if input == "/model" {
        return Ok(UiAction::ShowModels);
    }
    if input.starts_with("/provider ") {
        return Err("use `/provider` and choose from the picker".to_owned());
    }
    if input.starts_with("/model ") {
        return Err("use `/model` and choose from the picker".to_owned());
    }
    Err(format!("unknown command `{input}`"))
}

/// State rendered by Ratatui. All rendering derives only from these fields.
#[derive(Default)]
pub struct UiState {
    input: String,
    transcript: Vec<TranscriptRow>,
    provider_auth: BTreeMap<String, bool>,
    selected_model: Option<ModelRef>,
    active_submission: Option<SubmissionId>,
    scroll_offset: usize,
    should_exit: bool,
    mode: UiMode,
    highlighted_index: usize,
    provider_choices: Vec<(ProviderId, bool)>,
    detail_provider: Option<ProviderId>,
    model_choices: Vec<ModelInfo>,
    provider_operation: Option<(ProviderId, ProviderOperationKind)>,
    input_history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
}

impl UiState {
    pub fn transcript(&self) -> &[TranscriptRow] {
        &self.transcript
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn active_submission(&self) -> Option<SubmissionId> {
        self.active_submission
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub fn mode(&self) -> UiMode {
        self.mode
    }

    pub fn highlighted_index(&self) -> usize {
        self.highlighted_index
    }

    pub fn selected_model(&self) -> Option<ModelRef> {
        self.selected_model.clone()
    }

    pub fn picker_labels(&self) -> Vec<String> {
        match self.mode {
            UiMode::Input => Vec::new(),
            UiMode::ProviderList => self
                .provider_choices
                .iter()
                .map(|(provider, authenticated)| {
                    format!(
                        "{} {}",
                        provider.as_str(),
                        if *authenticated { "✓" } else { "○" }
                    )
                })
                .collect(),
            UiMode::ProviderDetail => self
                .detail_provider
                .as_ref()
                .map(|provider| {
                    vec![if let Some((_, operation)) = self
                        .provider_operation
                        .as_ref()
                        .filter(|(pending, _)| pending == provider)
                    {
                        operation.label().to_owned()
                    } else if self.provider_is_authenticated(provider) {
                        "Log out".to_owned()
                    } else {
                        "Authorize".to_owned()
                    }]
                })
                .unwrap_or_default(),
            UiMode::ModelList => self
                .model_choices
                .iter()
                .map(|model| {
                    format!(
                        "{}/{} — {}",
                        model.model.provider.as_str(),
                        model.model.model.as_str(),
                        model.display_name
                    )
                })
                .collect(),
        }
    }

    pub fn composer_input(&self) -> &str {
        &self.input
    }

    pub fn history_len(&self) -> usize {
        self.input_history.len()
    }

    /// Applies a local, side-effect-free state transition.
    pub fn reduce(&mut self, action: UiAction) {
        match action {
            UiAction::Noop => {}
            UiAction::AppendAssistantText(text) => {
                self.transcript.push(TranscriptRow::AssistantText(text))
            }
            UiAction::AppendToolCall { id, name } => {
                self.transcript.push(TranscriptRow::ToolCall { id, name });
            }
            UiAction::AppendToolResult { id, is_error } => {
                self.transcript
                    .push(TranscriptRow::ToolResult { id, is_error });
            }
            UiAction::ScrollUp => self.scroll_offset = self.scroll_offset.saturating_add(1),
            UiAction::ScrollDown => self.scroll_offset = self.scroll_offset.saturating_sub(1),
            UiAction::HistoryPrevious => self.recall_previous_input(),
            UiAction::HistoryNext => self.recall_next_input(),
            UiAction::PickerUp => self.move_highlight_up(),
            UiAction::PickerDown => self.move_highlight_down(),
            UiAction::PickerBack => self.back_from_picker(),
            UiAction::PickerConfirm => {}
            UiAction::CancelAndExit => self.should_exit = true,
            UiAction::ShowProviders
            | UiAction::StartAuth(_)
            | UiAction::ShowModels
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    fn open_provider_list(&mut self, providers: Vec<(ProviderId, bool)>) {
        self.provider_choices = providers;
        self.mode = UiMode::ProviderList;
        self.highlighted_index = 0;
        self.detail_provider = None;
    }

    fn open_provider_detail(&mut self) {
        self.detail_provider = self
            .provider_choices
            .get(self.highlighted_index)
            .map(|(provider, _)| provider.clone());
        if self.detail_provider.is_some() {
            self.mode = UiMode::ProviderDetail;
            self.highlighted_index = 0;
        }
    }

    fn open_model_list(&mut self, models: Vec<ModelInfo>) {
        self.model_choices = models;
        self.mode = UiMode::ModelList;
        self.highlighted_index = 0;
    }

    fn choice_count(&self) -> usize {
        match self.mode {
            UiMode::ProviderList => self.provider_choices.len(),
            UiMode::ProviderDetail => usize::from(self.detail_provider.is_some()),
            UiMode::ModelList => self.model_choices.len(),
            UiMode::Input => 0,
        }
    }

    fn move_highlight_up(&mut self) {
        let count = self.choice_count();
        if count > 0 {
            self.highlighted_index = (self.highlighted_index + count - 1) % count;
        }
    }

    fn move_highlight_down(&mut self) {
        let count = self.choice_count();
        if count > 0 {
            self.highlighted_index = (self.highlighted_index + 1) % count;
        }
    }

    fn back_from_picker(&mut self) {
        match self.mode {
            UiMode::ProviderDetail => {
                self.mode = UiMode::ProviderList;
                self.detail_provider = None;
                self.highlighted_index = 0;
            }
            UiMode::ProviderList | UiMode::ModelList => {
                self.mode = UiMode::Input;
                self.highlighted_index = 0;
            }
            UiMode::Input => {}
        }
    }

    fn provider_is_authenticated(&self, provider: &ProviderId) -> bool {
        self.provider_auth
            .get(provider.as_str())
            .copied()
            .unwrap_or(false)
    }

    fn composer_text(&self) -> String {
        if self.mode == UiMode::Input {
            return format!("› {}", self.input);
        }
        let labels = self.picker_labels();
        let rows = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                format!(
                    "{} {label}",
                    if index == self.highlighted_index {
                        "›"
                    } else {
                        " "
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        match self.mode {
            UiMode::ProviderList => format!("Select provider  ↑↓ Enter  Esc\n{rows}"),
            UiMode::ProviderDetail => {
                let provider = self
                    .detail_provider
                    .as_ref()
                    .map(ProviderId::as_str)
                    .unwrap_or("unknown");
                let status = self
                    .detail_provider
                    .as_ref()
                    .map(|provider| {
                        if self.provider_is_authenticated(provider) {
                            "authenticated ✓"
                        } else {
                            "not authenticated"
                        }
                    })
                    .unwrap_or("unavailable");
                format!("{provider}  {status}\n{rows}\nEsc back")
            }
            UiMode::ModelList => format!("Select model  ↑↓ Enter  Esc\n{rows}"),
            UiMode::Input => format!("› {}", self.input),
        }
    }

    fn push_input(&mut self, character: char) {
        self.detach_history_navigation();
        self.input.push(character);
    }

    fn pop_input(&mut self) {
        self.detach_history_navigation();
        self.input.pop();
    }

    fn take_submitted_input(&mut self) -> String {
        const MAX_HISTORY_ENTRIES: usize = 100;
        let input = std::mem::take(&mut self.input);
        self.history_index = None;
        self.history_draft = None;
        if !input.trim().is_empty()
            && self.input_history.last().map(String::as_str) != Some(input.as_str())
        {
            self.input_history.push(input.clone());
            if self.input_history.len() > MAX_HISTORY_ENTRIES {
                self.input_history.remove(0);
            }
        }
        input
    }

    fn detach_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn recall_previous_input(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft = Some(self.input.clone());
                self.input_history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.input.clone_from(&self.input_history[index]);
    }

    fn recall_next_input(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.input_history.len() {
            let next = index + 1;
            self.history_index = Some(next);
            self.input.clone_from(&self.input_history[next]);
        } else {
            self.input = self.history_draft.take().unwrap_or_default();
            self.history_index = None;
        }
    }

    fn add_error(&mut self, error: impl fmt::Display) {
        self.transcript
            .push(TranscriptRow::Error(error.to_string()));
    }

    fn finish_provider_operation(
        &mut self,
        provider: &ProviderId,
        kind: ProviderOperationKind,
    ) -> bool {
        if self.provider_operation.as_ref() == Some(&(provider.clone(), kind)) {
            self.provider_operation = None;
            true
        } else {
            false
        }
    }

    fn add_provider(&mut self, provider: ProviderId, authenticated: bool) {
        let id = provider.as_str().to_owned();
        self.provider_auth.insert(id, authenticated);
        if let Some((_, status)) = self
            .provider_choices
            .iter_mut()
            .find(|(choice, _)| choice == &provider)
        {
            *status = authenticated;
        }
    }

    fn add_models(&mut self, provider: ProviderId, models: Vec<ModelInfo>) {
        if self.mode == UiMode::ModelList {
            self.model_choices
                .retain(|model| model.model.provider != provider);
            self.model_choices.extend(models);
            self.highlighted_index = self
                .highlighted_index
                .min(self.model_choices.len().saturating_sub(1));
        }
    }

    fn set_selected_model(&mut self, model: ModelRef) {
        self.selected_model = Some(model);
    }

    fn apply_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ProviderDiscovered { provider } => {
                self.provider_auth
                    .entry(provider.as_str().to_owned())
                    .or_insert(false);
            }
            CoreEvent::AuthenticationChanged {
                provider,
                authenticated,
            } => self.add_provider(provider, authenticated),
            CoreEvent::ModelsListed { provider, models } => self.add_models(provider, models),
            CoreEvent::ModelSelected { model } => self.set_selected_model(model),
            CoreEvent::SubmissionStarted { submission, .. } => {
                self.active_submission = Some(submission)
            }
            CoreEvent::TextDelta { delta, .. } => self.reduce(UiAction::AppendAssistantText(delta)),
            CoreEvent::ToolCall { call, .. } => self.reduce(UiAction::AppendToolCall {
                id: call.id,
                name: call.name,
            }),
            CoreEvent::ToolResult { result, .. } => self.add_tool_result(result),
            CoreEvent::Completed { submission } => self.finish_submission(submission, "completed"),
            CoreEvent::Cancelled { submission } => self.finish_submission(submission, "cancelled"),
            CoreEvent::Failed {
                submission,
                message,
            } => {
                if self.active_submission == Some(submission) {
                    self.active_submission = None;
                }
                self.add_error(message);
            }
            CoreEvent::Shutdown => self.should_exit = true,
        }
    }

    fn add_tool_result(&mut self, result: ToolResult) {
        self.reduce(UiAction::AppendToolResult {
            id: result.tool_call_id,
            is_error: result.is_error,
        });
    }

    fn finish_submission(&mut self, submission: SubmissionId, status: &str) {
        if self.active_submission == Some(submission) {
            self.active_submission = None;
        }
        self.transcript
            .push(TranscriptRow::Info(format!("submission {status}")));
    }
}

/// Terminal-loop control result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiControl {
    Continue,
    Exit,
}

/// Errors returned by side effects initiated from UI actions.
#[derive(Debug)]
pub enum TuiError {
    Core(CoreError),
    Browser(String),
    AuthStartMissingUrl,
    AuthStartMissingSession,
    InvalidAuthUrl(String),
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "{error}"),
            Self::Browser(error) => formatter.write_str(error),
            Self::AuthStartMissingUrl => {
                formatter.write_str("provider auth.start response is missing a URL")
            }
            Self::AuthStartMissingSession => {
                formatter.write_str("provider auth.start response is missing a session")
            }
            Self::InvalidAuthUrl(error) => formatter.write_str(error),
        }
    }
}

impl Error for TuiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Browser(_)
            | Self::AuthStartMissingUrl
            | Self::AuthStartMissingSession
            | Self::InvalidAuthUrl(_) => None,
        }
    }
}

impl From<CoreError> for TuiError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

/// Thin TUI client that translates actions into calls on the headless core.
pub struct TuiClient<B> {
    core: MisyCore,
    events: Receiver<CoreEvent>,
    browser: B,
    state: UiState,
    provider_operation_sender: Sender<ProviderOperationResult>,
    provider_operation_results: Receiver<ProviderOperationResult>,
}

impl<B: BrowserHandoff> TuiClient<B> {
    pub fn new(core: MisyCore, browser: B) -> Self {
        let selected_model = core.selected_model();
        let (provider_operation_sender, provider_operation_results) = mpsc::channel();
        Self {
            events: core.subscribe_lossless(),
            core,
            browser,
            state: UiState {
                selected_model,
                ..UiState::default()
            },
            provider_operation_sender,
            provider_operation_results,
        }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    pub fn browser(&self) -> &B {
        &self.browser
    }

    pub fn running_provider_count(&self) -> usize {
        self.core.running_provider_count()
    }

    pub fn insert_text(&mut self, text: &str) {
        if self.state.mode == UiMode::Input {
            for character in text.chars() {
                self.state.push_input(character);
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.state.mode == UiMode::Input {
            self.state.pop_input();
        }
    }

    pub fn submit_composer(&mut self) -> Result<TuiControl, TuiError> {
        if self.state.mode != UiMode::Input {
            return Ok(TuiControl::Continue);
        }
        let input = self.state.take_submitted_input();
        self.handle_input(&input)
    }

    /// Parses and executes a line after the caller has collected it from the input widget.
    pub fn handle_input(&mut self, input: &str) -> Result<TuiControl, TuiError> {
        match map_input(input) {
            Ok(action) => match self.execute(action) {
                Ok(control) => Ok(control),
                Err(error) => {
                    self.state.add_error(&error);
                    Err(error)
                }
            },
            Err(error) => {
                self.state.add_error(&error);
                Ok(TuiControl::Continue)
            }
        }
    }

    pub fn handle_key(&mut self, key: UiKey) -> Result<TuiControl, TuiError> {
        match self.execute(map_key(self.state.mode, key)) {
            Ok(control) => Ok(control),
            Err(error) => {
                self.state.add_error(&error);
                Err(error)
            }
        }
    }

    /// Drains received core events without blocking the terminal event loop.
    pub fn pump_events(&mut self) -> usize {
        const MAX_EVENTS_PER_TICK: usize = 256;
        let mut received = 0;
        while received < MAX_EVENTS_PER_TICK {
            match self.events.try_recv() {
                Ok(event) => {
                    received += 1;
                    self.state.apply_core_event(event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        while let Ok(result) = self.provider_operation_results.try_recv() {
            match result {
                ProviderOperationResult::Status(provider, result) => {
                    if !self
                        .state
                        .finish_provider_operation(&provider, ProviderOperationKind::Status)
                    {
                        continue;
                    }
                    match result {
                        Ok(authenticated) => self.state.add_provider(provider, authenticated),
                        Err(error) => self.state.add_error(error),
                    }
                }
                ProviderOperationResult::Start(provider, result) => {
                    if !self
                        .state
                        .finish_provider_operation(&provider, ProviderOperationKind::Start)
                    {
                        continue;
                    }
                    match result {
                        Ok(result) => {
                            if let Err(error) = self.open_authorization(provider, result) {
                                self.state.add_error(error);
                            }
                        }
                        Err(error) => self.state.add_error(error),
                    }
                }
                ProviderOperationResult::Complete(provider, result) => {
                    if !self
                        .state
                        .finish_provider_operation(&provider, ProviderOperationKind::Complete)
                    {
                        continue;
                    }
                    match result {
                        Ok(model) => {
                            self.state.add_provider(provider, true);
                            self.state.set_selected_model(model);
                        }
                        Err(error) => self.state.add_error(error),
                    }
                }
            }
        }
        received
    }

    /// Ctrl+C always attempts cancellation and shutdown before requesting terminal exit.
    pub fn handle_ctrl_c(&mut self) -> TuiControl {
        if let Some(submission) = self.state.active_submission
            && let Err(error) = self.core.cancel(submission)
        {
            self.state.add_error(error);
        }
        if let Err(error) = self.core.shutdown() {
            self.state.add_error(error);
        }
        self.state.reduce(UiAction::CancelAndExit);
        TuiControl::Exit
    }

    fn open_authorization(&mut self, provider: ProviderId, result: Value) -> Result<(), TuiError> {
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .ok_or(TuiError::AuthStartMissingUrl)?;
        let session = result
            .get("session")
            .cloned()
            .ok_or(TuiError::AuthStartMissingSession)?;
        validate_authorization_url(url).map_err(TuiError::InvalidAuthUrl)?;
        self.browser.open(url).map_err(TuiError::Browser)?;
        self.state.provider_operation = Some((provider.clone(), ProviderOperationKind::Complete));
        self.state.transcript.push(TranscriptRow::Info(format!(
            "authorization opened for {}",
            provider.as_str()
        )));
        let core = self.core.clone();
        let sender = self.provider_operation_sender.clone();
        thread::spawn(move || {
            let result = core
                .complete_auth(&provider, session)
                .and_then(|_| core.select_default_model(&provider))
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::Complete(provider, result));
        });
        Ok(())
    }

    fn execute(&mut self, action: UiAction) -> Result<TuiControl, TuiError> {
        match action {
            UiAction::Noop => Ok(TuiControl::Continue),
            UiAction::ShowProviders => {
                let mut choices = Vec::new();
                for provider in self.core.providers() {
                    let authenticated = self.core.has_credentials(&provider.id)?;
                    choices.push((provider.id, authenticated));
                }
                self.state.open_provider_list(choices);
                Ok(TuiControl::Continue)
            }
            UiAction::StartAuth(provider) => {
                self.state.provider_operation =
                    Some((provider.clone(), ProviderOperationKind::Start));
                let core = self.core.clone();
                let sender = self.provider_operation_sender.clone();
                thread::spawn(move || {
                    let result = core
                        .start_auth(&provider)
                        .map_err(|error| error.to_string());
                    let _ = sender.send(ProviderOperationResult::Start(provider, result));
                });
                Ok(TuiControl::Continue)
            }
            UiAction::ShowModels => {
                self.state.open_model_list(self.core.available_models()?);
                Ok(TuiControl::Continue)
            }
            UiAction::SelectModel(model) => {
                self.core.select_model(model)?;
                self.state.mode = UiMode::Input;
                Ok(TuiControl::Continue)
            }
            UiAction::SubmitPrompt(prompt) => {
                let submission = self.core.submit(Message::user(prompt.clone()))?;
                self.state
                    .transcript
                    .push(TranscriptRow::UserPrompt(prompt));
                self.state.active_submission = Some(submission);
                Ok(TuiControl::Continue)
            }
            UiAction::AppendAssistantText(_)
            | UiAction::AppendToolCall { .. }
            | UiAction::AppendToolResult { .. }
            | UiAction::ScrollUp
            | UiAction::ScrollDown
            | UiAction::HistoryPrevious
            | UiAction::HistoryNext
            | UiAction::PickerUp
            | UiAction::PickerDown
            | UiAction::PickerBack => {
                self.state.reduce(action);
                Ok(TuiControl::Continue)
            }
            UiAction::PickerConfirm => self.confirm_picker(),
            UiAction::CancelAndExit => Ok(self.handle_ctrl_c()),
        }
    }

    fn confirm_picker(&mut self) -> Result<TuiControl, TuiError> {
        match self.state.mode {
            UiMode::Input => Ok(TuiControl::Continue),
            UiMode::ProviderList => {
                if let Some(provider) = self
                    .state
                    .provider_choices
                    .get(self.state.highlighted_index)
                    .map(|(provider, _)| provider.clone())
                {
                    self.state.provider_operation =
                        Some((provider.clone(), ProviderOperationKind::Status));
                    let core = self.core.clone();
                    let sender = self.provider_operation_sender.clone();
                    thread::spawn(move || {
                        let result = core
                            .auth_status(&provider)
                            .map(|result| {
                                result
                                    .get("authenticated")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false)
                            })
                            .map_err(|error| error.to_string());
                        let _ = sender.send(ProviderOperationResult::Status(provider, result));
                    });
                }
                self.state.open_provider_detail();
                Ok(TuiControl::Continue)
            }
            UiMode::ProviderDetail => {
                let Some(provider) = self.state.detail_provider.clone() else {
                    return Ok(TuiControl::Continue);
                };
                if self
                    .state
                    .provider_operation
                    .as_ref()
                    .is_some_and(|(pending, _)| pending == &provider)
                {
                    return Ok(TuiControl::Continue);
                }
                if self.state.provider_is_authenticated(&provider) {
                    self.core.logout(&provider)?;
                    self.state.add_provider(provider, false);
                    Ok(TuiControl::Continue)
                } else {
                    self.execute(UiAction::StartAuth(provider))
                }
            }
            UiMode::ModelList => {
                let Some(model) = self
                    .state
                    .model_choices
                    .get(self.state.highlighted_index)
                    .map(|choice| choice.model.clone())
                else {
                    return Ok(TuiControl::Continue);
                };
                self.execute(UiAction::SelectModel(model))
            }
        }
    }
}

/// Starts the interactive Ratatui client.
pub fn run(core: MisyCore) -> Result<(), io::Error> {
    let stdout = io::stdout();
    let mut guard = TerminalGuard::enter(&stdout)?;
    let mut client = TuiClient::new(core, SystemBrowser);

    let terminal_result = (|| {
        let backend = CrosstermBackend::new(stdout.lock());
        let mut terminal = Terminal::new(backend)?;
        while !client.state().should_exit() {
            terminal.draw(|frame| render(frame, client.state()))?;
            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    client.handle_ctrl_c();
                } else if client.state().mode != UiMode::Input {
                    let picker_key = match key.code {
                        KeyCode::Up => Some(UiKey::Up),
                        KeyCode::Down => Some(UiKey::Down),
                        KeyCode::Enter => Some(UiKey::Enter),
                        KeyCode::Esc => Some(UiKey::Escape),
                        _ => None,
                    };
                    if let Some(picker_key) = picker_key {
                        let _ = client.handle_key(picker_key);
                    }
                } else {
                    match key.code {
                        KeyCode::Enter => {
                            let _ = client.submit_composer();
                        }
                        KeyCode::Backspace => client.backspace(),
                        KeyCode::Char(character) => {
                            let mut encoded = [0; 4];
                            client.insert_text(character.encode_utf8(&mut encoded));
                        }
                        KeyCode::Up => {
                            let _ = client.handle_key(UiKey::Up);
                        }
                        KeyCode::Down => {
                            let _ = client.handle_key(UiKey::Down);
                        }
                        KeyCode::PageUp => {
                            let _ = client.handle_key(UiKey::PageUp);
                        }
                        KeyCode::PageDown => {
                            let _ = client.handle_key(UiKey::PageDown);
                        }
                        _ => {}
                    }
                }
            }
            client.pump_events();
        }
        Ok(())
    })();
    client.handle_ctrl_c();
    guard.restore();
    terminal_result
}

/// Renders the unboxed transcript, one separator, and one-line input.
pub fn render(frame: &mut ratatui::Frame, state: &UiState) {
    const WORDMARK: &str = "███   ███  █████  █████  █   █\n████ ████    █    █       █ █\n██ ███ ██    █     ███     █\n██  █  ██    █        █    █\n██     ██  █████  █████    █\n                 MISY";
    let area = frame.area();
    let provider = state
        .selected_model
        .as_ref()
        .map(|model| model.provider.as_str())
        .or_else(|| state.provider_auth.keys().next().map(String::as_str))
        .unwrap_or("none");
    let model = state
        .selected_model
        .as_ref()
        .map(|model| model.model.as_str())
        .unwrap_or("none");
    let transcript = state
        .transcript
        .iter()
        .map(TranscriptRow::display)
        .collect::<Vec<_>>()
        .join("\n");
    let content = if transcript.is_empty() {
        format!("{WORDMARK}\nagent: misy | provider: {provider} | model: {model}")
    } else {
        format!("{WORDMARK}\nagent: misy | provider: {provider} | model: {model}\n\n{transcript}")
    };
    let transcript = Paragraph::new(content).wrap(Wrap { trim: false });
    let composer = Paragraph::new(state.composer_text()).wrap(Wrap { trim: false });
    let rendered_lines = transcript.line_count(area.width);
    let composer_height = u16::try_from(composer.line_count(area.width))
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(1));
    let content_capacity = area
        .height
        .saturating_sub(1)
        .saturating_sub(composer_height);
    let content_height = u16::try_from(rendered_lines)
        .unwrap_or(u16::MAX)
        .min(content_capacity);
    let maximum_scroll = rendered_lines.saturating_sub(usize::from(content_height));
    let scroll = maximum_scroll.saturating_sub(state.scroll_offset.min(maximum_scroll));
    let content_area = Rect::new(area.x, area.y, area.width, content_height);
    let separator_area = Rect::new(
        area.x,
        area.y.saturating_add(content_height),
        area.width,
        u16::from(area.height > content_height),
    );
    let input_area = Rect::new(
        area.x,
        separator_area.y.saturating_add(separator_area.height),
        area.width,
        composer_height,
    );
    frame.render_widget(
        transcript.scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(separator_area.width))),
        separator_area,
    );
    frame.render_widget(composer, input_area);
    if state.mode == UiMode::Input && input_area.height > 0 && input_area.width > 0 {
        let cursor_offset = 2usize.saturating_add(Line::from(state.input.as_str()).width());
        let cursor_x = input_area
            .x
            .saturating_add(u16::try_from(cursor_offset).unwrap_or(u16::MAX))
            .min(input_area.right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, input_area.y));
    }
}

struct TerminalGuard<'a> {
    stdout: &'a Stdout,
    restored: bool,
}

impl<'a> TerminalGuard<'a> {
    fn enter(stdout: &'a Stdout) -> Result<Self, io::Error> {
        enable_raw_mode()?;
        if let Err(error) = execute!(stdout.lock(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            stdout,
            restored: false,
        })
    }

    fn restore(&mut self) {
        if !self.restored {
            let _ = disable_raw_mode();
            let _ = execute!(self.stdout.lock(), LeaveAlternateScreen);
            self.restored = true;
        }
    }
}

impl Drop for TerminalGuard<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}
