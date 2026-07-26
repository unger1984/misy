//! Thin TUI client over the headless core.

mod authentication;
mod interrupts;
mod snapshot;

use super::{
    action::{UiAction, UiKey, UiMode},
    browser::BrowserHandoff,
    history::PromptHistoryStore,
    state::{ProviderAction, ProviderOperationKind, UiState},
};
use misy_core::{
    AvailableModels, CoreError, CoreEvent, Message, MisyCore, MisyPaths, ModelRef, ProviderId,
    ProviderManifest, SubmissionId, UsageReport,
};
use serde_json::Value;
use snapshot::provider_choices;
use std::{collections::BTreeMap, error::Error, fmt, time::Duration};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::TryRecvError};

const MAX_EVENTS_PER_TICK: usize = 256;
const MAX_OPERATIONS_PER_TICK: usize = 64;
const QUIT_SHORTCUT_TIMEOUT: Duration = Duration::from_secs(1);

/// Terminal-loop control result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiControl {
    /// Continue processing terminal events.
    Continue,
    /// Restore the terminal and exit.
    Exit,
}

/// Errors returned by side effects initiated from UI actions.
#[derive(Debug)]
pub enum TuiError {
    /// A headless-core operation failed.
    Core(CoreError),
    /// The operating system rejected a browser handoff.
    Browser(String),
    /// A provider returned an invalid or unsupported `auth.start` response.
    InvalidAuthStart(String),
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => write!(formatter, "{error}"),
            Self::Browser(error) | Self::InvalidAuthStart(error) => formatter.write_str(error),
        }
    }
}

impl Error for TuiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for TuiError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

pub(super) enum ProviderOperationResult {
    Start(ProviderId, String, Result<Value, String>),
    Complete(ProviderId, String, Result<(), String>),
    Logout(ProviderId, Result<(), String>),
    Models(u64, Result<AvailableModels, String>),
    SelectModel(ProviderId, Result<ModelRef, String>),
    Usage(ModelRef, Result<UsageReport, String>),
    Submit(String, Result<SubmissionId, String>),
}

/// Thin interactive client that translates input into headless core operations.
pub struct TuiClient<B> {
    pub(super) core: MisyCore,
    events: UnboundedReceiver<CoreEvent>,
    submission_sender: UnboundedSender<String>,
    browser: B,
    pub(super) state: UiState,
    pub(super) operation_sender: UnboundedSender<ProviderOperationResult>,
    operation_results: UnboundedReceiver<ProviderOperationResult>,
    provider_manifests: Vec<ProviderManifest>,
    pub(super) cached_models: AvailableModels,
    provider_names: BTreeMap<String, String>,
    prompt_history: Option<PromptHistoryStore>,
    pub(super) model_refresh_generation: u64,
}

impl<B: BrowserHandoff> TuiClient<B> {
    /// Creates a client without starting provider processes.
    pub async fn new(core: MisyCore, browser: B) -> Self {
        Self::build(core, browser, None).await
    }

    /// Creates a client that recalls accepted prompts from the user's Misy data directory.
    pub async fn with_persistent_history(core: MisyCore, browser: B, paths: &MisyPaths) -> Self {
        Self::build(core, browser, Some(PromptHistoryStore::new(paths))).await
    }

    async fn build(core: MisyCore, browser: B, prompt_history: Option<PromptHistoryStore>) -> Self {
        let snapshot = core.snapshot();
        let provider_manifests = core.providers().await;
        let provider_names: BTreeMap<String, String> = provider_manifests
            .iter()
            .cloned()
            .map(|provider| (provider.id.as_str().to_owned(), provider.display_name))
            .collect();
        let cached_models = core.cached_available_models().await.unwrap_or_default();
        let (operation_sender, operation_results) = mpsc::unbounded_channel();
        let (submission_sender, submission_requests) = mpsc::unbounded_channel();
        spawn_submission_worker(core.clone(), operation_sender.clone(), submission_requests);
        let mut state = UiState::default();
        let working_directory = std::env::current_dir().ok();
        state.set_startup_header(
            snapshot.selected_model.as_ref(),
            working_directory.as_deref(),
        );
        if let Some(history) = &prompt_history {
            match history.load() {
                Ok(entries) => state.composer = super::composer::Composer::with_history(entries),
                Err(error) => state.add_error(format!("could not load prompt history: {error}")),
            }
        }
        state.apply_snapshot(snapshot);
        state.set_provider_names(provider_names.clone());
        Self {
            events: core.subscribe_lossless(),
            core,
            submission_sender,
            browser,
            state,
            operation_sender,
            operation_results,
            provider_manifests,
            cached_models,
            provider_names,
            prompt_history,
            model_refresh_generation: 0,
        }
    }

    /// Returns deterministic render state.
    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// Returns the browser adapter, primarily for observing test handoffs.
    pub fn browser(&self) -> &B {
        &self.browser
    }

    /// Returns the number of lazily started provider processes.
    pub async fn running_provider_count(&self) -> usize {
        self.core.running_provider_count().await
    }

    /// Inserts text into the focused composer or modal filter.
    pub fn insert_text(&mut self, text: &str) {
        self.state.clear_quit_shortcut();
        if self.state.mode() == UiMode::Input {
            self.state.composer.insert_str(text);
        } else {
            self.state.insert_filter(text);
        }
    }

    /// Inserts one terminal paste without interpreting embedded newlines as submissions.
    pub fn paste_text(&mut self, text: &str) {
        self.state.clear_quit_shortcut();
        let normalized = normalize_paste(text);
        if self.state.mode() == UiMode::Input {
            self.state.composer.insert_str(&normalized);
        } else {
            self.state
                .insert_filter(&normalized.replace(['\n', '\t'], " "));
        }
    }

    pub(super) fn position_composer_cursor(&mut self, row: u16, column: u16) {
        self.state.clear_quit_shortcut();
        if self.state.mode() == UiMode::Input {
            self.state.composer.position_cursor(row, column);
        }
    }

    /// Inserts a newline without submitting the composer.
    pub fn insert_newline(&mut self) {
        if self.state.mode() == UiMode::Input {
            self.state.composer.insert_newline();
        }
    }

    /// Deletes the character before the focused cursor.
    pub fn backspace(&mut self) {
        if self.state.mode() == UiMode::Input {
            self.state.composer.backspace();
        } else {
            self.state.backspace_filter();
        }
    }

    pub(super) fn report_terminal_error(&mut self, error: impl fmt::Display) {
        self.state.add_error(error);
    }

    /// Submits the composer or accepts its slash-command completion.
    ///
    /// # Errors
    ///
    /// Returns core or browser-handoff errors produced by the selected action.
    pub fn submit_composer(&mut self) -> Result<TuiControl, TuiError> {
        if self.state.mode() != UiMode::Input {
            return Ok(TuiControl::Continue);
        }
        if self.state.composer.popup_visible()
            && let Some(command) = self.state.composer.selected_command()
        {
            let command = command.to_owned();
            self.state.composer.clear();
            let (result, accepted) = self.handle_submitted_input(&command);
            if accepted {
                self.record_prompt_history(&command);
            }
            return result;
        }
        let input = self.state.composer.take_text();
        let (result, accepted) = self.handle_submitted_input(&input);
        if accepted {
            self.record_prompt_history(&input);
        }
        result
    }

    /// Parses and executes submitted text.
    ///
    /// # Errors
    ///
    /// Returns failures from core calls initiated by the resulting action.
    pub fn handle_input(&mut self, input: &str) -> Result<TuiControl, TuiError> {
        self.state.clear_quit_shortcut();
        self.handle_submitted_input(input).0
    }

    fn handle_submitted_input(&mut self, input: &str) -> (Result<TuiControl, TuiError>, bool) {
        match super::action::map_input(input) {
            Ok(UiAction::Noop) => (Ok(TuiControl::Continue), false),
            Ok(action) => {
                let result = self
                    .execute(action)
                    .inspect_err(|error| self.state.add_error(error));
                let accepted = result.is_ok();
                (result, accepted)
            }
            Err(error) => {
                self.state.add_error(error);
                (Ok(TuiControl::Continue), false)
            }
        }
    }

    fn record_prompt_history(&mut self, text: &str) {
        if !self.state.composer.record_submitted(text) {
            return;
        }
        if let Some(history) = &self.prompt_history
            && let Err(error) = history.append(text)
        {
            self.state
                .add_error(format!("could not save prompt history: {error}"));
        }
    }

    /// Routes a normalized key through modal view, popup, then composer layers.
    ///
    /// # Errors
    ///
    /// Returns failures from an accepted selection or submitted prompt.
    pub fn handle_key(&mut self, key: UiKey) -> Result<TuiControl, TuiError> {
        self.state.clear_quit_shortcut();
        if self.state.mode() != UiMode::Input {
            return self.handle_view_key(key);
        }
        if key == UiKey::Escape {
            self.refresh_snapshot();
            if let Some(submission) = self.state.interruptible_submission() {
                self.handle_escape(submission);
                return Ok(TuiControl::Continue);
            }
            if !self.state.composer.popup_visible() {
                return Ok(TuiControl::Continue);
            }
        }
        if self.state.composer.popup_visible() {
            match key {
                UiKey::Up => self.state.composer.popup_up(),
                UiKey::Down => self.state.composer.popup_down(),
                UiKey::Escape => self.state.composer.dismiss_popup(),
                UiKey::Enter => return self.submit_composer(),
                UiKey::Tab => {
                    self.state.composer.complete_selected_command();
                }
                _ => return self.handle_composer_key(key),
            }
            return Ok(TuiControl::Continue);
        }
        self.handle_composer_key(key)
    }

    /// Drains bounded core and worker result queues without blocking input.
    pub fn pump_events(&mut self) -> usize {
        let mut received = 0;
        while received < MAX_EVENTS_PER_TICK {
            match self.events.try_recv() {
                Ok(event) => {
                    received += 1;
                    if snapshot::requires_refresh(&event) {
                        self.refresh_snapshot();
                    }
                    self.state.apply_core_event(event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        for _ in 0..MAX_OPERATIONS_PER_TICK {
            let Ok(result) = self.operation_results.try_recv() else {
                break;
            };
            self.apply_operation_result(result);
        }
        received
    }

    fn handle_view_key(&mut self, key: UiKey) -> Result<TuiControl, TuiError> {
        match key {
            UiKey::Up => self.state.reduce(UiAction::PickerUp),
            UiKey::Down => self.state.reduce(UiAction::PickerDown),
            UiKey::Left => self.state.reduce(UiAction::PickerTabLeft),
            UiKey::Right => self.state.reduce(UiAction::PickerTabRight),
            UiKey::Backspace => self.state.backspace_filter(),
            UiKey::Escape => {
                if matches!(
                    self.state.provider_operation,
                    Some((
                        _,
                        ProviderOperationKind::Start
                            | ProviderOperationKind::Complete
                            | ProviderOperationKind::Logout
                            | ProviderOperationKind::SelectModel
                    ))
                ) {
                    return Ok(TuiControl::Continue);
                }
                self.state.reduce(UiAction::PickerBack);
            }
            UiKey::Enter => return self.confirm_view(),
            UiKey::SelectIndex(index) => {
                if self.state.select_picker_number(index) {
                    return self.confirm_view();
                }
            }
            _ => {}
        }
        Ok(TuiControl::Continue)
    }

    fn handle_composer_key(&mut self, key: UiKey) -> Result<TuiControl, TuiError> {
        match key {
            UiKey::Left => self.state.composer.move_left(),
            UiKey::Right => self.state.composer.move_right(),
            UiKey::Home => self.state.composer.move_home(),
            UiKey::End => self.state.composer.move_end(),
            UiKey::Backspace => self.state.composer.backspace(),
            UiKey::Delete => self.state.composer.delete(),
            UiKey::Newline => self.state.composer.insert_newline(),
            UiKey::Up => self.state.composer.history_previous(),
            UiKey::Down => self.state.composer.history_next(),
            UiKey::PageUp | UiKey::PageDown => {}
            UiKey::Enter => return self.submit_composer(),
            UiKey::Escape | UiKey::Tab | UiKey::SelectIndex(_) => {}
        }
        Ok(TuiControl::Continue)
    }

    fn execute(&mut self, action: UiAction) -> Result<TuiControl, TuiError> {
        match action {
            UiAction::ShowProviders => self.show_providers()?,
            UiAction::ShowModels => self.start_model_refresh(),
            UiAction::ShowUsage => self.show_usage()?,
            UiAction::SubmitPrompt(prompt) => self.submit_prompt(prompt),
            UiAction::SelectModel(model) => self.select_model(model),
            UiAction::StartAuth(provider) => {
                let method = self
                    .provider_manifests
                    .iter()
                    .cloned()
                    .into_iter()
                    .find(|manifest| manifest.id == provider)
                    .and_then(|manifest| manifest.auth_methods.into_iter().next())
                    .map(|method| method.id)
                    .unwrap_or_default();
                self.start_auth(provider, method);
            }
            UiAction::PickerConfirm => return self.confirm_view(),
            UiAction::CancelAndExit => return Ok(self.exit_now()),
            action => self.state.reduce(action),
        }
        Ok(TuiControl::Continue)
    }

    fn show_providers(&mut self) -> Result<(), TuiError> {
        self.state.open_providers(provider_choices(
            &self.provider_manifests,
            &self.core.snapshot(),
        ));
        Ok(())
    }

    fn show_usage(&mut self) -> Result<(), TuiError> {
        let Some(model) = self.core.snapshot().selected_model else {
            self.state.add_error(CoreError::NoModelSelected);
            return Ok(());
        };
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn({
            let requested_model = model;
            async move {
                let result = core
                    .usage(&requested_model)
                    .await
                    .map_err(|error| error.to_string());
                let _ = sender.send(ProviderOperationResult::Usage(requested_model, result));
            }
        });
        Ok(())
    }

    fn submit_prompt(&mut self, prompt: String) {
        self.state.queue_prompt_submission(prompt.clone());
        if self.submission_sender.send(prompt).is_err() {
            self.state.discard_last_pending_submission();
            self.state
                .add_error("prompt submission worker stopped unexpectedly");
        }
    }

    fn confirm_view(&mut self) -> Result<TuiControl, TuiError> {
        match self.state.mode() {
            UiMode::ProviderList => {
                if let Some(provider) = self.state.selected_provider() {
                    self.state.open_provider_settings(&provider);
                }
            }
            UiMode::ProviderDetail => {
                if self.state.provider_operation.is_some() {
                    return Ok(TuiControl::Continue);
                }
                if let Some((provider, action)) = self.state.selected_provider_action() {
                    match action {
                        ProviderAction::Authorize(method) => self.start_auth(provider, method),
                        ProviderAction::Logout => self.logout(provider),
                    }
                }
            }
            UiMode::ModelList => {
                if let Some(model) = self.state.selected_model_choice() {
                    self.select_model(model);
                }
            }
            UiMode::Input => {}
        }
        Ok(TuiControl::Continue)
    }

    fn start_auth(&mut self, provider: ProviderId, method: String) {
        self.state
            .set_provider_operation(provider.clone(), ProviderOperationKind::Start, None);
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .start_auth_with_method(&provider, &method)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::Start(provider, method, result));
        });
    }

    fn logout(&mut self, provider: ProviderId) {
        self.state
            .set_provider_operation(provider.clone(), ProviderOperationKind::Logout, None);
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .logout(&provider)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::Logout(provider, result));
        });
    }

    fn select_model(&mut self, model: ModelRef) {
        self.state.set_provider_operation(
            model.provider.clone(),
            ProviderOperationKind::SelectModel,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let provider = model.provider.clone();
        tokio::spawn(async move {
            let result = core
                .select_model(model.clone())
                .await
                .map(|()| model)
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::SelectModel(provider, result));
        });
    }

    fn apply_operation_result(&mut self, result: ProviderOperationResult) {
        match result {
            ProviderOperationResult::Models(generation, result) => {
                if generation != self.model_refresh_generation {
                    return;
                }
                match result {
                    Ok(available) => {
                        self.cached_models = available.clone();
                        self.state.finish_models(available, &self.provider_names);
                    }
                    Err(error) => {
                        let provider = ProviderId::new("models");
                        if self
                            .state
                            .finish_provider_operation(&provider, ProviderOperationKind::Models)
                        {
                            self.state.add_error(error);
                        }
                    }
                }
            }
            ProviderOperationResult::Start(provider, method, result) => {
                if !self
                    .state
                    .finish_provider_operation(&provider, ProviderOperationKind::Start)
                {
                    return;
                }
                match result {
                    Ok(auth) => {
                        if let Err(error) = self.open_authorization(provider, method, &auth) {
                            self.state.add_error(error);
                        }
                    }
                    Err(error) => self.state.add_error(error),
                }
            }
            ProviderOperationResult::Complete(provider, _method, result) => {
                if !self
                    .state
                    .finish_provider_operation(&provider, ProviderOperationKind::Complete)
                {
                    return;
                }
                match result {
                    Ok(()) => self.refresh_snapshot(),
                    Err(error) => self.state.add_error(error),
                }
            }
            ProviderOperationResult::Logout(provider, result) => {
                if !self
                    .state
                    .finish_provider_operation(&provider, ProviderOperationKind::Logout)
                {
                    return;
                }
                match result {
                    Ok(()) => self.refresh_snapshot(),
                    Err(error) => self.state.add_error(error),
                }
            }
            ProviderOperationResult::SelectModel(provider, result) => {
                if !self
                    .state
                    .finish_provider_operation(&provider, ProviderOperationKind::SelectModel)
                {
                    return;
                }
                match result {
                    Ok(model) => {
                        self.refresh_snapshot();
                        self.state.view = None;
                        self.state
                            .add_info(format!("model: {}", model.model.as_str()));
                    }
                    Err(error) => self.state.add_error(error),
                }
            }
            ProviderOperationResult::Usage(model, result) => match result {
                Ok(report) => {
                    let provider = self.provider_display_name(&model.provider).to_owned();
                    for line in super::usage::format_usage_report(&provider, &report) {
                        self.state.add_info(line);
                    }
                }
                Err(error) => self.state.add_error(error),
            },
            ProviderOperationResult::Submit(prompt, result) => {
                self.apply_submit_result(prompt, result)
            }
        }
    }

    fn apply_submit_result(&mut self, prompt: String, result: Result<SubmissionId, String>) {
        match result {
            Ok(submission) => {
                self.state.accept_submission(submission, prompt);
                self.refresh_snapshot();
            }
            Err(error) => {
                self.state.reject_submission(&prompt);
                self.state.add_error(error);
            }
        }
    }

    fn provider_display_name<'a>(&'a self, provider: &'a ProviderId) -> &'a str {
        self.provider_names
            .get(provider.as_str())
            .map(String::as_str)
            .unwrap_or(provider.as_str())
    }
}

fn spawn_submission_worker(
    core: MisyCore,
    operation_sender: UnboundedSender<ProviderOperationResult>,
    mut submission_requests: UnboundedReceiver<String>,
) {
    tokio::spawn(async move {
        while let Some(prompt) = submission_requests.recv().await {
            let result = core
                .submit(Message::user(prompt.clone()))
                .await
                .map_err(|error| error.to_string());
            let _ = operation_sender.send(ProviderOperationResult::Submit(prompt, result));
        }
    });
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}
