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
    widgets::{Paragraph, Wrap},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{self, Stdout},
    process::Command,
    sync::mpsc::{Receiver, TryRecvError},
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
    CompleteAuth(ProviderId, Value),
    ShowModels,
    SelectModel(ModelRef),
    SubmitPrompt(String),
    AppendAssistantText(String),
    AppendToolCall { id: String, name: String },
    AppendToolResult { id: String, is_error: bool },
    ScrollUp,
    ScrollDown,
    CancelAndExit,
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
    if let Some(rest) = input.strip_prefix("/provider ") {
        let mut parts = rest.splitn(3, ' ');
        let provider = parts.next().filter(|part| !part.is_empty());
        let command = parts.next();
        let remainder = parts.next();
        let Some(provider) = provider else {
            return Err("provider id is required".to_owned());
        };
        return match (command, remainder) {
            (Some("auth"), None) => Ok(UiAction::StartAuth(ProviderId::new(provider))),
            (Some("complete"), Some(completion)) => serde_json::from_str(completion)
                .map(|value| UiAction::CompleteAuth(ProviderId::new(provider), value))
                .map_err(|error| format!("invalid auth completion JSON: {error}")),
            _ => Err(
                "use `/provider`, `/provider <id> auth`, or `/provider <id> complete <json>`"
                    .to_owned(),
            ),
        };
    }
    if let Some(reference) = input.strip_prefix("/model ") {
        let Some((provider, model)) = reference.split_once('/') else {
            return Err("model must be written as <provider>/<model>".to_owned());
        };
        if provider.is_empty() || model.is_empty() || model.contains('/') {
            return Err("model must be written as <provider>/<model>".to_owned());
        }
        return Ok(UiAction::SelectModel(ModelRef::new(
            ProviderId::new(provider),
            crate::ModelId::new(model),
        )));
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
            UiAction::CancelAndExit => self.should_exit = true,
            UiAction::ShowProviders
            | UiAction::StartAuth(_)
            | UiAction::CompleteAuth(_, _)
            | UiAction::ShowModels
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    fn push_input(&mut self, character: char) {
        self.input.push(character);
    }

    fn pop_input(&mut self) {
        self.input.pop();
    }

    fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input)
    }

    fn add_error(&mut self, error: impl fmt::Display) {
        self.transcript
            .push(TranscriptRow::Error(error.to_string()));
    }

    fn add_provider(&mut self, provider: ProviderId, authenticated: bool) {
        let id = provider.as_str().to_owned();
        self.provider_auth.insert(id.clone(), authenticated);
        self.transcript
            .push(TranscriptRow::Provider { id, authenticated });
    }

    fn add_models(&mut self, models: Vec<ModelInfo>) {
        for info in models {
            self.transcript.push(TranscriptRow::Model {
                provider: info.model.provider.as_str().to_owned(),
                id: info.model.model.as_str().to_owned(),
                selected: self.selected_model.as_ref() == Some(&info.model),
            });
        }
    }

    fn set_selected_model(&mut self, model: ModelRef) {
        self.selected_model = Some(model.clone());
        self.transcript.push(TranscriptRow::Model {
            provider: model.provider.as_str().to_owned(),
            id: model.model.as_str().to_owned(),
            selected: true,
        });
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
            CoreEvent::ModelsListed { models, .. } => self.add_models(models),
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
            Self::InvalidAuthUrl(error) => formatter.write_str(error),
        }
    }
}

impl Error for TuiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Browser(_) | Self::AuthStartMissingUrl | Self::InvalidAuthUrl(_) => None,
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
}

impl<B: BrowserHandoff> TuiClient<B> {
    pub fn new(core: MisyCore, browser: B) -> Self {
        let selected_model = core.selected_model();
        Self {
            events: core.subscribe_lossless(),
            core,
            browser,
            state: UiState {
                selected_model,
                ..UiState::default()
            },
        }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    pub fn browser(&self) -> &B {
        &self.browser
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

    fn execute(&mut self, action: UiAction) -> Result<TuiControl, TuiError> {
        match action {
            UiAction::Noop => Ok(TuiControl::Continue),
            UiAction::ShowProviders => {
                for provider in self.core.providers() {
                    self.core.auth_status(&provider.id)?;
                }
                Ok(TuiControl::Continue)
            }
            UiAction::StartAuth(provider) => {
                let result = self.core.start_auth(&provider)?;
                let url = result
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or(TuiError::AuthStartMissingUrl)?;
                validate_authorization_url(url).map_err(TuiError::InvalidAuthUrl)?;
                self.browser.open(url).map_err(TuiError::Browser)?;
                self.state.transcript.push(TranscriptRow::Info(format!(
                    "authorization opened for {}; finish with /provider {} complete <json>",
                    provider.as_str(),
                    provider.as_str()
                )));
                Ok(TuiControl::Continue)
            }
            UiAction::CompleteAuth(provider, completion) => {
                self.core.complete_auth(&provider, completion)?;
                Ok(TuiControl::Continue)
            }
            UiAction::ShowModels => {
                for provider in self.core.providers() {
                    self.core.list_models(&provider.id)?;
                }
                Ok(TuiControl::Continue)
            }
            UiAction::SelectModel(model) => {
                self.core.select_model(model)?;
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
            | UiAction::ScrollDown => {
                self.state.reduce(action);
                Ok(TuiControl::Continue)
            }
            UiAction::CancelAndExit => Ok(self.handle_ctrl_c()),
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
                } else {
                    match key.code {
                        KeyCode::Enter => {
                            let input = client.state.take_input();
                            let _ = client.handle_input(&input);
                        }
                        KeyCode::Backspace => client.state.pop_input(),
                        KeyCode::Char(character) => client.state.push_input(character),
                        KeyCode::Up => client.state.reduce(UiAction::ScrollUp),
                        KeyCode::Down => client.state.reduce(UiAction::ScrollDown),
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
    let rendered_lines = transcript.line_count(area.width);
    let content_capacity = area.height.saturating_sub(2);
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
        u16::from(area.height > content_height.saturating_add(separator_area.height)),
    );
    frame.render_widget(
        transcript.scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(separator_area.width))),
        separator_area,
    );
    frame.render_widget(Paragraph::new(state.input.as_str()), input_area);
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
