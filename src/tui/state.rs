//! Deterministic state rendered by the terminal client.

use super::{
    action::{UiAction, UiMode},
    composer::Composer,
    list::{ListRow, ListView},
};
use crate::{
    AvailableModels, CoreEvent, ModelRef, ProviderAuthMethod, ProviderId, SubmissionId, ToolResult,
};
use std::{collections::BTreeMap, fmt, time::Instant};

/// A renderable, user-visible transcript item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptRow {
    /// Provider discovery/status information retained for compatible clients.
    Provider {
        /// Stable provider identifier.
        id: String,
        /// Whether local credentials exist.
        authenticated: bool,
    },
    /// Model discovery information retained for compatible clients.
    Model {
        /// Stable provider identifier.
        provider: String,
        /// Provider-scoped model identifier.
        id: String,
        /// Whether this model is currently selected.
        selected: bool,
    },
    /// Submitted user prompt.
    UserPrompt(String),
    /// One streamed assistant text delta.
    AssistantText(String),
    /// Tool execution start.
    ToolCall {
        /// Provider tool-call identifier.
        id: String,
        /// Registered tool name.
        name: String,
        /// Compact JSON arguments supplied by the provider, when available.
        arguments: Option<String>,
    },
    /// Tool execution completion.
    ToolResult {
        /// Provider tool-call identifier.
        id: String,
        /// Whether the tool failed.
        is_error: bool,
        /// The user-visible local tool result, when available.
        content: Option<String>,
    },
    /// Informational lifecycle message.
    Info(String),
    /// User-visible failure.
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProviderChoice {
    pub(super) id: ProviderId,
    pub(super) display_name: String,
    pub(super) authenticated: bool,
    pub(super) credential_method: Option<String>,
    pub(super) auth_methods: Vec<ProviderAuthMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ProviderAction {
    Authorize(String),
    Logout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ActiveView {
    Providers(ListView<ProviderId>),
    ProviderSettings {
        provider: ProviderId,
        display_name: String,
        credential_method: Option<String>,
        actions: ListView<ProviderAction>,
    },
    Models(ListView<ModelRef>),
}

/// Bottom-pane data derived from one active modal view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ModalPresentation {
    pub(super) title: String,
    pub(super) rows: Vec<super::list::ListRowDisplay>,
    pub(super) operation: Option<String>,
    pub(super) back_hint: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProviderOperationKind {
    Start,
    Complete,
    Logout,
    Models,
    SelectModel,
}

impl ProviderOperationKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Start => "Opening browser…",
            Self::Complete => "Waiting for browser…",
            Self::Logout => "Logging out…",
            Self::Models => "Loading models…",
            Self::SelectModel => "Selecting model…",
        }
    }
}

/// State rendered by Ratatui. Rendering depends only on this value.
#[derive(Default)]
pub struct UiState {
    pub(super) composer: Composer,
    transcript: Vec<TranscriptRow>,
    providers: BTreeMap<String, ProviderChoice>,
    provider_names: BTreeMap<String, String>,
    selected_model: Option<ModelRef>,
    active_submission: Option<SubmissionId>,
    submission_started_at: Option<Instant>,
    should_exit: bool,
    pub(super) view: Option<ActiveView>,
    pub(super) provider_operation: Option<(ProviderId, ProviderOperationKind)>,
}

impl UiState {
    /// Returns the transcript rows in display order.
    pub fn transcript(&self) -> &[TranscriptRow] {
        &self.transcript
    }

    /// Returns the active submission, if a model turn is running.
    pub fn active_submission(&self) -> Option<SubmissionId> {
        self.active_submission
    }

    /// Returns whether the terminal loop should exit.
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    /// Returns the currently visible bottom-pane surface.
    pub fn mode(&self) -> UiMode {
        match self.view {
            None => UiMode::Input,
            Some(ActiveView::Providers(_)) => UiMode::ProviderList,
            Some(ActiveView::ProviderSettings { .. }) => UiMode::ProviderDetail,
            Some(ActiveView::Models(_)) => UiMode::ModelList,
        }
    }

    /// Returns the selected list index for compatibility with deterministic tests.
    pub fn highlighted_index(&self) -> usize {
        self.picker_labels()
            .iter()
            .position(|label| label.starts_with("› "))
            .unwrap_or(0)
    }

    /// Returns the selected provider-scoped model.
    pub fn selected_model(&self) -> Option<ModelRef> {
        self.selected_model.clone()
    }

    /// Returns visible picker labels without starting provider processes.
    pub fn picker_labels(&self) -> Vec<String> {
        match &self.view {
            None => Vec::new(),
            Some(ActiveView::Providers(view)) => view.labels(),
            Some(ActiveView::ProviderSettings { actions, .. }) => {
                if let Some((_, operation)) = self.provider_operation {
                    vec![operation.label().to_owned()]
                } else {
                    actions.labels()
                }
            }
            Some(ActiveView::Models(view)) => view.labels(),
        }
    }

    /// Returns the current composer draft.
    pub fn composer_input(&self) -> &str {
        self.composer.text()
    }

    /// Returns the byte cursor position in the composer draft.
    pub fn composer_cursor(&self) -> usize {
        self.composer.cursor()
    }

    /// Returns the number of retained prompt-history entries.
    pub fn history_len(&self) -> usize {
        self.composer.history_len()
    }

    /// Returns whether the non-modal slash-command popup is visible.
    pub fn command_popup_visible(&self) -> bool {
        self.view.is_none() && self.composer.popup_visible()
    }

    /// Returns rendered slash-command popup rows.
    pub fn command_popup_rows(&self) -> Vec<String> {
        if self.view.is_some() {
            return Vec::new();
        }
        self.composer.popup_rows()
    }

    /// Applies a local, side-effect-free state transition.
    pub fn reduce(&mut self, action: UiAction) {
        match action {
            UiAction::Noop => {}
            UiAction::AppendAssistantText(text) => {
                self.append_assistant_text(text);
            }
            UiAction::AppendToolCall { id, name } => {
                self.transcript.push(TranscriptRow::ToolCall {
                    id,
                    name,
                    arguments: None,
                });
            }
            UiAction::AppendToolResult { id, is_error } => {
                self.transcript.push(TranscriptRow::ToolResult {
                    id,
                    is_error,
                    content: None,
                });
            }
            UiAction::HistoryPrevious => self.composer.history_previous(),
            UiAction::HistoryNext => self.composer.history_next(),
            UiAction::PickerUp => self.move_picker_up(),
            UiAction::PickerDown => self.move_picker_down(),
            UiAction::PickerBack => self.back_from_picker(),
            UiAction::CancelAndExit => self.should_exit = true,
            UiAction::PickerConfirm
            | UiAction::ShowProviders
            | UiAction::StartAuth(_)
            | UiAction::ShowModels
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    pub(super) fn add_error(&mut self, error: impl fmt::Display) {
        self.transcript
            .push(TranscriptRow::Error(error.to_string()));
    }

    pub(super) fn add_info(&mut self, message: impl Into<String>) {
        self.transcript.push(TranscriptRow::Info(message.into()));
    }

    pub(super) fn push_transcript(&mut self, row: TranscriptRow) {
        self.transcript.push(row);
    }

    pub(super) fn set_active_submission(&mut self, submission: Option<SubmissionId>) {
        self.active_submission = submission;
        self.submission_started_at = self.active_submission.as_ref().map(|_| Instant::now());
    }

    pub(super) fn set_selected_model(&mut self, model: Option<ModelRef>) {
        self.selected_model = model;
    }

    pub(super) fn open_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.providers = providers
            .into_iter()
            .map(|provider| (provider.id.as_str().to_owned(), provider))
            .collect();
        self.view = Some(ActiveView::Providers(self.provider_list()));
    }

    pub(super) fn open_provider_settings(&mut self, provider: &ProviderId) {
        let Some(choice) = self.providers.get(provider.as_str()).cloned() else {
            return;
        };
        self.view = Some(provider_settings(choice));
    }

    pub(super) fn open_loading_models(&mut self) {
        self.provider_operation = Some((ProviderId::new("models"), ProviderOperationKind::Models));
        self.view = Some(ActiveView::Models(ListView::new(
            "Select model",
            vec![ListRow::informational("Loading models…")],
        )));
    }

    pub(super) fn finish_models(
        &mut self,
        available: AvailableModels,
        names: &BTreeMap<String, String>,
    ) {
        if !matches!(self.view, Some(ActiveView::Models(_)))
            || !matches!(
                self.provider_operation,
                Some((_, ProviderOperationKind::Models))
            )
        {
            return;
        }
        self.provider_operation = None;
        let mut rows = available
            .models
            .into_iter()
            .map(|model| {
                let display_name = model.display_name;
                let provider = names
                    .get(model.model.provider.as_str())
                    .cloned()
                    .unwrap_or_else(|| model.model.provider.as_str().to_owned());
                let selected = self.selected_model.as_ref() == Some(&model.model);
                let label = model.model.model.as_str().to_owned();
                if selected {
                    ListRow::current_with_search(model.model, label, Some(provider), display_name)
                } else {
                    ListRow::selectable_with_search(
                        model.model,
                        label,
                        Some(provider),
                        display_name,
                    )
                }
            })
            .collect::<Vec<_>>();
        rows.extend(available.errors.into_iter().map(|error| {
            ListRow::informational(format!(
                "{} — error: {}",
                error.provider_display_name, error.message
            ))
        }));
        self.view = Some(ActiveView::Models(ListView::new("Select model", rows)));
    }

    pub(super) fn set_provider_authenticated(
        &mut self,
        provider: &ProviderId,
        authenticated: bool,
        method: Option<String>,
    ) {
        if let Some(choice) = self.providers.get_mut(provider.as_str()) {
            choice.authenticated = authenticated;
            choice.credential_method = method;
        }
        match &self.view {
            Some(ActiveView::Providers(_)) => {
                self.view = Some(ActiveView::Providers(self.provider_list()));
            }
            Some(ActiveView::ProviderSettings {
                provider: active, ..
            }) if active == provider => self.open_provider_settings(provider),
            _ => {}
        }
    }

    pub(super) fn finish_provider_operation(
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

    pub(super) fn apply_core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::ProviderDiscovered { .. } | CoreEvent::ModelsListed { .. } => {}
            CoreEvent::AuthenticationChanged {
                provider,
                authenticated,
            } => self.set_provider_authenticated(&provider, authenticated, None),
            CoreEvent::ModelSelected { model } => self.selected_model = Some(model),
            CoreEvent::SubmissionStarted { submission, .. } => {
                self.set_active_submission(Some(submission))
            }
            CoreEvent::TextDelta { delta, .. } => self.reduce(UiAction::AppendAssistantText(delta)),
            CoreEvent::ToolCall { call, .. } => self.transcript.push(TranscriptRow::ToolCall {
                id: call.id,
                name: call.name,
                arguments: serde_json::to_string(&call.arguments).ok(),
            }),
            CoreEvent::ToolResult { result, .. } => self.add_tool_result(result),
            CoreEvent::Completed { submission } => self.finish_submission(submission, "completed"),
            CoreEvent::Cancelled { submission } => self.finish_submission(submission, "cancelled"),
            CoreEvent::Failed {
                submission,
                message,
            } => {
                if self.active_submission == Some(submission) {
                    self.set_active_submission(None);
                }
                self.add_error(message);
            }
            CoreEvent::Shutdown => self.should_exit = true,
        }
    }

    pub(super) fn modal_presentation(&self, visible_rows: usize) -> Option<ModalPresentation> {
        match &self.view {
            None => None,
            Some(ActiveView::Providers(view)) => Some(list_presentation(
                view,
                visible_rows,
                self.provider_operation.as_ref().map(|(_, kind)| *kind),
            )),
            Some(ActiveView::Models(view)) => Some(list_presentation(
                view,
                visible_rows,
                self.provider_operation.as_ref().map(|(_, kind)| *kind),
            )),
            Some(ActiveView::ProviderSettings {
                display_name,
                credential_method,
                actions,
                ..
            }) => {
                let status = credential_method
                    .as_deref()
                    .map(|method| format!("authenticated via {method}"))
                    .unwrap_or_else(|| "not authenticated".to_owned());
                Some(ModalPresentation {
                    title: format!("{display_name} — {status}"),
                    rows: actions.visible_rows(visible_rows),
                    operation: self
                        .provider_operation
                        .as_ref()
                        .map(|(_, kind)| kind.label().to_owned()),
                    back_hint: true,
                })
            }
        }
    }

    pub(super) fn set_provider_names(&mut self, names: BTreeMap<String, String>) {
        self.provider_names = names;
    }

    pub(super) fn status_text(&self) -> String {
        let Some(model) = &self.selected_model else {
            return "model not selected".to_owned();
        };
        let provider = self
            .provider_names
            .get(model.provider.as_str())
            .map(String::as_str)
            .unwrap_or(model.provider.as_str());
        format!("{provider} · {}", model.model.as_str())
    }

    pub(super) fn composer_cursor_position(&self) -> (u16, u16) {
        self.composer.cursor_position()
    }

    pub(super) fn composer_line_count(&self) -> usize {
        self.composer.text().split('\n').count()
    }

    pub(super) fn command_popup_rows_for_render(&self) -> Vec<super::composer::CommandPopupRow> {
        if self.view.is_some() {
            return Vec::new();
        }
        self.composer.popup_rows_for_render()
    }

    pub(super) fn busy_label(&self, now: Instant) -> Option<String> {
        let started = self.submission_started_at?;
        let elapsed = now.saturating_duration_since(started);
        let frame = spinner_frame(elapsed.as_millis() / 100);
        Some(format!(
            "{frame} Working… ({}s · esc to interrupt)",
            elapsed.as_secs()
        ))
    }

    /// Returns the contiguous finalized prefix suitable for terminal scrollback.
    ///
    /// The active response and tool activity remain in the inline viewport until the core marks
    /// the submission complete, preventing a delta-by-delta trail in terminal history.
    pub(super) fn finalized_transcript_len(&self) -> usize {
        let Some(_) = self.active_submission else {
            return self.transcript.len();
        };
        let prompt_end = self
            .transcript
            .iter()
            .rposition(|row| matches!(row, TranscriptRow::UserPrompt(_)))
            .map_or(0, |index| index + 1);
        self.transcript[prompt_end..]
            .iter()
            .rposition(|row| matches!(row, TranscriptRow::ToolResult { .. }))
            .map_or(prompt_end, |index| prompt_end + index + 1)
    }

    /// Returns only the currently changing response/tool rows for the inline viewport.
    pub(super) fn live_transcript(&self) -> &[TranscriptRow] {
        if self.active_submission.is_none() {
            return &[];
        }
        let first_live = self.finalized_transcript_len();
        &self.transcript[first_live..]
    }

    fn provider_list(&self) -> ListView<ProviderId> {
        let rows = self
            .providers
            .values()
            .map(|provider| {
                ListRow::selectable(
                    provider.id.clone(),
                    provider.display_name.clone(),
                    Some(if provider.authenticated {
                        "✓ authenticated".to_owned()
                    } else {
                        "not authenticated".to_owned()
                    }),
                )
            })
            .collect();
        ListView::new("Providers", rows)
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.insert_filter(text),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.insert_filter(text),
            Some(ActiveView::Models(view)) => view.insert_filter(text),
            None => {}
        }
    }

    pub(super) fn backspace_filter(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.backspace_filter(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.backspace_filter(),
            Some(ActiveView::Models(view)) => view.backspace_filter(),
            None => {}
        }
    }

    pub(super) fn selected_provider(&self) -> Option<ProviderId> {
        match &self.view {
            Some(ActiveView::Providers(view)) => view.selected_value().cloned(),
            _ => None,
        }
    }

    pub(super) fn selected_provider_action(&self) -> Option<(ProviderId, ProviderAction)> {
        match &self.view {
            Some(ActiveView::ProviderSettings {
                provider, actions, ..
            }) => actions
                .selected_value()
                .cloned()
                .map(|action| (provider.clone(), action)),
            _ => None,
        }
    }

    pub(super) fn selected_model_choice(&self) -> Option<ModelRef> {
        match &self.view {
            Some(ActiveView::Models(view)) => view.selected_value().cloned(),
            _ => None,
        }
    }

    pub(super) fn select_picker_number(&mut self, one_based: usize) -> bool {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.select_number(one_based),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.select_number(one_based),
            Some(ActiveView::Models(view)) => view.select_number(one_based),
            None => false,
        }
    }

    fn move_picker_up(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_up(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_up(),
            Some(ActiveView::Models(view)) => view.move_up(),
            None => {}
        }
    }

    fn move_picker_down(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_down(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_down(),
            Some(ActiveView::Models(view)) => view.move_down(),
            None => {}
        }
    }

    fn back_from_picker(&mut self) {
        match &self.view {
            Some(ActiveView::ProviderSettings { .. }) => {
                self.view = Some(ActiveView::Providers(self.provider_list()));
            }
            Some(ActiveView::Providers(_) | ActiveView::Models(_)) => {
                self.view = None;
                self.provider_operation = None;
            }
            None => {}
        }
    }

    fn add_tool_result(&mut self, result: ToolResult) {
        self.transcript.push(TranscriptRow::ToolResult {
            id: result.tool_call_id,
            is_error: result.is_error,
            content: Some(result.content),
        });
    }

    fn append_assistant_text(&mut self, text: String) {
        if let Some(TranscriptRow::AssistantText(previous)) = self.transcript.last_mut() {
            previous.push_str(&text);
            return;
        }
        self.transcript.push(TranscriptRow::AssistantText(text));
    }

    fn finish_submission(&mut self, submission: SubmissionId, status: &str) {
        if self.active_submission == Some(submission) {
            self.set_active_submission(None);
        }
        self.add_info(format!("submission {status}"));
    }
}

fn spinner_frame(ticks: u128) -> &'static str {
    match ticks % 10 {
        0 => "⠋",
        1 => "⠙",
        2 => "⠹",
        3 => "⠸",
        4 => "⠼",
        5 => "⠴",
        6 => "⠦",
        7 => "⠧",
        8 => "⠇",
        _ => "⠏",
    }
}

fn list_presentation<T>(
    view: &ListView<T>,
    visible_rows: usize,
    operation: Option<ProviderOperationKind>,
) -> ModalPresentation {
    ModalPresentation {
        title: view.title().to_owned(),
        rows: view.visible_rows(visible_rows),
        operation: operation.map(|kind| kind.label().to_owned()),
        back_hint: false,
    }
}

fn provider_settings(provider: ProviderChoice) -> ActiveView {
    let credential_method = provider.credential_method.clone();
    let rows = if provider.authenticated {
        vec![ListRow::selectable(ProviderAction::Logout, "Log out", None)]
    } else {
        provider
            .auth_methods
            .iter()
            .map(|method| {
                ListRow::selectable(
                    ProviderAction::Authorize(method.id.clone()),
                    "Authorize",
                    Some(method.display_name.clone()),
                )
            })
            .collect()
    };
    ActiveView::ProviderSettings {
        provider: provider.id,
        display_name: provider.display_name,
        credential_method,
        actions: ListView::new("Provider settings", rows),
    }
}
