//! Thin TUI client over the headless core.

mod authentication;
mod interrupts;
mod operation_result;
mod sessions;
mod snapshot;
mod submission;

use super::{
    action::{UiAction, UiKey, UiMode},
    browser::BrowserHandoff,
    composer::ComposerSnapshot,
    composer_attachment::ComposerDraft,
    history::PromptHistoryStore,
    keymap::Keymap,
    state::{OperationScope, ProviderAction, ProviderOperationKind, UiState},
};
use misy_core::{
    ActivityId, ActivityOutput, AvailableModels, CoreError, CoreEvent, MisyCore, MisyPaths,
    ModelRef, ProviderDisplayName, ProviderId, ProviderManifest, SubmissionId, UsageReport,
};
use serde_json::Value;
use snapshot::provider_choices;
use std::{collections::BTreeMap, error::Error, fmt, time::Duration};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::TryRecvError};
use tokio::task::AbortHandle;

const MAX_EVENTS_PER_TICK: usize = 256;
const MAX_OPERATIONS_PER_TICK: usize = 64;
const QUIT_SHORTCUT_TIMEOUT: Duration = Duration::from_secs(1);

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
    CancelAuth(ProviderId, Result<(), String>),
    Logout(ProviderId, Result<(), String>),
    Models(u64, Result<AvailableModels, String>),
    SelectModel(ProviderId, Result<ModelRef, String>),
    Usage(ModelRef, Result<UsageReport, String>),
    // `SubmissionAccepted` maps the transcript; this result gates draft clearing and history.
    Submit(SubmissionRequest, Result<SubmissionId, String>),
    ActivityOutput(ActivityId, Option<ActivityOutput>),
}

pub(super) struct SubmissionRequest {
    pub(super) draft: ComposerDraft,
    pub(super) clear_composer: bool,
    pub(super) history_text: Option<String>,
    pub(super) history_snapshot: Option<ComposerSnapshot>,
}

/// Thin interactive client that translates input into headless core operations.
pub struct TuiClient<B> {
    pub(super) core: MisyCore,
    events: UnboundedReceiver<CoreEvent>,
    submission_sender: UnboundedSender<SubmissionRequest>,
    browser: B,
    pub(super) state: UiState,
    pub(super) operation_sender: UnboundedSender<ProviderOperationResult>,
    operation_results: UnboundedReceiver<ProviderOperationResult>,
    provider_manifests: Vec<ProviderManifest>,
    pub(super) cached_models: AvailableModels,
    provider_names: BTreeMap<ProviderId, ProviderDisplayName>,
    prompt_history: Option<PromptHistoryStore>,
    pub(super) model_refresh_generation: u64,
    auth_task: Option<(ProviderId, ProviderOperationKind, AbortHandle)>,
    composer_submission_pending: bool,
    activity_output_pending: bool,
    next_activity_output_refresh: std::time::Instant,
    keymap: Keymap,
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
        let events = core.subscribe_lossless();
        let snapshot = core.snapshot();
        let (keymap, keymap_warnings) = Keymap::from_overrides(&core.keybindings());
        let provider_manifests = core.providers().await;
        let provider_names: BTreeMap<ProviderId, ProviderDisplayName> = provider_manifests
            .iter()
            .cloned()
            .map(|provider| (provider.id, provider.display_name))
            .collect();
        let cached_models = core.cached_available_models().await.unwrap_or_default();
        let (operation_sender, operation_results) = mpsc::unbounded_channel();
        let (submission_sender, submission_requests) = mpsc::unbounded_channel();
        operation_result::spawn_submission_worker(
            core.clone(),
            operation_sender.clone(),
            submission_requests,
        );
        let mut state = UiState::default();
        state.activity_stop_hint = keymap.stop_hint().to_owned();
        state.transcript_expand_hint = keymap.expand_hint().to_owned();
        for warning in keymap_warnings {
            state.add_error(warning);
        }
        // The header simply omits the directory when the process cwd is no longer readable.
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
            events,
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
            auth_task: None,
            composer_submission_pending: false,
            activity_output_pending: false,
            next_activity_output_refresh: std::time::Instant::now(),
            keymap,
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
        self.state.activity_bar_focused = false;
        if self.state.mode() == UiMode::Input {
            if !self.composer_submission_pending {
                self.state.composer.insert_str(text);
            }
        } else {
            self.state.insert_filter(text);
        }
    }

    /// Inserts one terminal paste without interpreting embedded newlines as submissions.
    pub fn paste_text(&mut self, text: &str) {
        self.state.clear_quit_shortcut();
        self.state.activity_bar_focused = false;
        let normalized = normalize_paste(text);
        if self.state.mode() == UiMode::Input {
            if !self.composer_submission_pending {
                self.state.composer.insert_str(&normalized);
            }
        } else {
            self.state
                .insert_filter(&normalized.replace(['\n', '\t'], " "));
        }
    }

    pub(super) fn position_composer_cursor(&mut self, row: u16, column: u16) {
        self.state.clear_quit_shortcut();
        self.state.activity_bar_focused = false;
        if self.state.mode() == UiMode::Input {
            self.state.composer.position_cursor(row, column);
        }
    }

    /// Inserts a newline without submitting the composer.
    pub fn insert_newline(&mut self) {
        if self.state.mode() == UiMode::Input && !self.composer_submission_pending {
            self.state.composer.insert_newline();
        }
    }

    /// Deletes the character before the focused cursor.
    pub fn backspace(&mut self) {
        if self.state.mode() == UiMode::Input {
            if !self.composer_submission_pending {
                self.state.composer.backspace();
            }
        } else {
            self.state.backspace_filter();
        }
    }

    pub(super) fn report_terminal_error(&mut self, error: impl fmt::Display) {
        self.state.add_error(error);
    }

    pub(super) fn configured_key(&self, key: crossterm::event::KeyEvent) -> Option<UiKey> {
        self.keymap.resolve(key)
    }
    /// Routes a normalized key through modal view, popup, then composer layers.
    ///
    /// # Errors
    ///
    /// Returns failures from an accepted selection or submitted prompt.
    pub fn handle_key(&mut self, key: UiKey) -> Result<(), TuiError> {
        self.state.clear_quit_shortcut();
        if key == UiKey::OpenActivities {
            self.state.open_activities();
            return Ok(());
        }
        if key == UiKey::StopActivity {
            self.stop_selected_activity();
            return Ok(());
        }
        if key == UiKey::ToggleToolOutput {
            self.state.toggle_tool_output();
            return Ok(());
        }
        if self.state.activity_bar_focused {
            match key {
                UiKey::Up | UiKey::Escape => self.state.activity_bar_focused = false,
                UiKey::Enter => self.state.open_activities(),
                _ => {}
            }
            return Ok(());
        }
        if self.state.mode() != UiMode::Input {
            return self.handle_view_key(key);
        }
        if key == UiKey::Escape {
            self.refresh_core_projection();
            if let Some(submission) = self.state.interruptible_submission() {
                self.handle_escape(submission);
                return Ok(());
            }
            if !self.state.composer.popup_visible() {
                return Ok(());
            }
        }
        if self.composer_submission_pending && composer_key_mutates_draft(key) {
            return Ok(());
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
            return Ok(());
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
                        if snapshot::affects_provider_choices(&event) {
                            self.refresh_provider_choices();
                        } else {
                            self.refresh_core_projection();
                        }
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
        self.refresh_activity_output_if_due();
        received
    }

    fn handle_view_key(&mut self, key: UiKey) -> Result<(), TuiError> {
        if self.state.mode() == UiMode::ActivityDetail {
            match key {
                UiKey::Up => self.state.scroll_activity_log_up(false),
                UiKey::Down => self.state.scroll_activity_log_down(false),
                UiKey::PageUp => self.state.scroll_activity_log_up(true),
                UiKey::PageDown => self.state.scroll_activity_log_down(true),
                UiKey::Escape => self.state.reduce(&UiAction::PickerBack),
                UiKey::StopActivity => self.stop_selected_activity(),
                _ => {}
            }
            return Ok(());
        }
        match key {
            UiKey::Up => self.state.reduce(&UiAction::PickerUp),
            UiKey::Down => self.state.reduce(&UiAction::PickerDown),
            UiKey::Left => self.state.reduce(&UiAction::PickerTabLeft),
            UiKey::Right => self.state.reduce(&UiAction::PickerTabRight),
            UiKey::Backspace => self.state.backspace_filter(),
            UiKey::Escape => {
                if self.cancel_active_authentication() {
                    return Ok(());
                }
                if matches!(
                    self.state.provider_operation,
                    Some((
                        _,
                        ProviderOperationKind::CancelAuth
                            | ProviderOperationKind::Logout
                            | ProviderOperationKind::SelectModel
                    ))
                ) {
                    return Ok(());
                }
                self.state.reduce(&UiAction::PickerBack);
            }
            UiKey::Enter => return self.confirm_view(),
            UiKey::SelectIndex(index) => {
                if self.state.select_picker_number(index) {
                    return self.confirm_view();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_composer_key(&mut self, key: UiKey) -> Result<(), TuiError> {
        match key {
            UiKey::Left => self.state.composer.move_left(),
            UiKey::Right => self.state.composer.move_right(),
            UiKey::Home => self.state.composer.move_home(),
            UiKey::End => self.state.composer.move_end(),
            UiKey::Backspace => self.state.composer.backspace(),
            UiKey::Delete => self.state.composer.delete(),
            UiKey::Newline => self.state.composer.insert_newline(),
            UiKey::Up => self.state.composer.history_previous(),
            UiKey::Down
                if self.state.composer_input().is_empty() && self.state.activity_bar_visible() =>
            {
                self.state.activity_bar_focused = true;
            }
            UiKey::Down => self.state.composer.history_next(),
            UiKey::PageUp | UiKey::PageDown => {}
            UiKey::Enter => return self.submit_composer(),
            UiKey::Escape
            | UiKey::Tab
            | UiKey::SelectIndex(_)
            | UiKey::OpenActivities
            | UiKey::StopActivity
            | UiKey::ToggleToolOutput => {}
        }
        Ok(())
    }

    fn execute(&mut self, action: UiAction) -> Result<(), TuiError> {
        match action {
            UiAction::ShowProviders => self.show_providers()?,
            UiAction::ShowModels => self.start_model_refresh(),
            UiAction::ShowActivities => self.state.open_activities(),
            UiAction::ShowSessions => self.show_sessions()?,
            UiAction::NewSession => self.start_new_session()?,
            UiAction::ResumeSession(id) => self.resume_session(&id)?,
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
            UiAction::CancelAndExit => self.exit_now(),
            action => self.state.reduce(&action),
        }
        Ok(())
    }

    fn start_activity_output_refresh(&mut self, id: ActivityId) {
        if self.activity_output_pending {
            return;
        }
        self.activity_output_pending = true;
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let output = core.activity_output(id, None).await;
            let _ = sender.send(ProviderOperationResult::ActivityOutput(id, output));
        });
    }

    fn refresh_activity_output_if_due(&mut self) {
        let Some(id) = self.state.activity_output_target() else {
            self.activity_output_pending = false;
            return;
        };
        let now = std::time::Instant::now();
        if now >= self.next_activity_output_refresh {
            self.next_activity_output_refresh = now + Duration::from_millis(200);
            self.start_activity_output_refresh(id);
        }
    }

    fn stop_selected_activity(&mut self) {
        let Some(id) = self.state.selected_activity_to_stop() else {
            return;
        };
        let refresh_detail = self.state.activity_detail_id() == Some(id);
        if !self.core.stop_activity(id) {
            self.state
                .add_error(format!("task `{id}` is already finished"));
        }
        if refresh_detail {
            self.start_activity_output_refresh(id);
        }
    }

    fn show_providers(&mut self) -> Result<(), TuiError> {
        self.state.open_providers(provider_choices(
            &self.provider_manifests,
            &self.core.snapshot(),
        ));
        Ok(())
    }

    fn confirm_view(&mut self) -> Result<(), TuiError> {
        match self.state.mode() {
            UiMode::ProviderList => {
                if let Some(provider) = self.state.selected_provider() {
                    self.state.open_provider_settings(&provider);
                }
            }
            UiMode::ProviderDetail => {
                if self.state.provider_operation.is_some() {
                    return Ok(());
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
            UiMode::ActivityList => match self.state.selected_activity_choice() {
                Some(super::activity_picker::ActivityChoice::Main) => self.state.view = None,
                Some(super::activity_picker::ActivityChoice::Activity(id)) => {
                    self.state.open_activity_detail(id);
                    self.start_activity_output_refresh(id);
                }
                None => {}
            },
            UiMode::SessionList => {
                if let Some(id) = self.state.selected_session_id() {
                    self.resume_session(&id)?;
                }
            }
            UiMode::ActivityDetail => {}
            UiMode::Input => {}
        }
        Ok(())
    }

    fn provider_display_name<'a>(&'a self, provider: &'a ProviderId) -> &'a str {
        self.provider_names
            .get(provider)
            .map(ProviderDisplayName::as_str)
            .unwrap_or(provider.as_str())
    }

    pub(super) fn track_auth_task(
        &mut self,
        provider: ProviderId,
        kind: ProviderOperationKind,
        task: AbortHandle,
    ) {
        self.auth_task = Some((provider, kind, task));
    }

    pub(super) fn finish_auth_task(&mut self, provider: &ProviderId, kind: ProviderOperationKind) {
        if self
            .auth_task
            .as_ref()
            .is_some_and(|(active, active_kind, _)| active == provider && *active_kind == kind)
        {
            self.auth_task = None;
        }
    }

    fn cancel_active_authentication(&mut self) -> bool {
        let Some((OperationScope::Provider(provider), kind)) =
            self.state.provider_operation.clone()
        else {
            return false;
        };
        if !matches!(
            kind,
            ProviderOperationKind::Start | ProviderOperationKind::Complete
        ) {
            return false;
        }
        let Some((task_provider, task_kind, task)) = self.auth_task.take() else {
            return false;
        };
        if task_provider != provider || task_kind != kind {
            self.auth_task = Some((task_provider, task_kind, task));
            return false;
        }
        task.abort();
        self.state.finish_provider_operation(&provider, kind);
        self.state.set_provider_operation(
            provider.clone(),
            ProviderOperationKind::CancelAuth,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .cancel_authentication(&provider)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::CancelAuth(provider, result));
        });
        true
    }
}

fn composer_key_mutates_draft(key: UiKey) -> bool {
    matches!(
        key,
        UiKey::Backspace | UiKey::Delete | UiKey::Newline | UiKey::Up | UiKey::Down | UiKey::Tab
    )
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}
