//! Deterministic state rendered by the terminal client.

mod activities;
mod context;
mod events;
mod input;
mod models;
mod providers;
mod questions;
mod sessions;
mod spinner;
#[cfg(test)]
mod tests;
mod transcript;
mod turns;

pub(super) use activities::output_content_labels;
pub(super) use spinner::spinner_frame;
pub use transcript::TranscriptRow;

use super::{
    action::{UiAction, UiMode},
    activity_picker::ActivityPicker,
    auth_prompt::AuthPromptView,
    composer::Composer,
    context_view::ContextView,
    list::ListView,
    model_picker::ModelPicker,
    presentation::{
        list_presentation, model_picker_presentation, operation_label, provider_settings,
    },
    question_dialog::QuestionDialog,
    session_picker::SessionPicker,
    startup_header::StartupHeader,
};
use misy_core::{
    ActivityOutput, AgentTranscript, CoreSnapshot, ModelProfile, ModelRef, ProviderAuthMethod,
    ProviderDisplayName, ProviderId, SubmissionId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProviderChoice {
    pub(super) id: ProviderId,
    pub(super) display_name: ProviderDisplayName,
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
        display_name: ProviderDisplayName,
        credential_method: Option<String>,
        actions: ListView<ProviderAction>,
    },
    AuthPrompt(AuthPromptView),
    Models(ModelPicker),
    Thinking(ListView<ModelProfile>),
    Activities(ActivityPicker),
    Sessions(SessionPicker),
    AgentDiscard(ListView<bool>),
    ActivityLog(Box<activities::ActivityLogView>),
    Question(QuestionDialog),
    Context(ContextView),
}

/// Bottom-pane data derived from one active modal view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ModalPresentation {
    pub(super) title: String,
    pub(super) rows: Vec<super::list::ListRowDisplay>,
    pub(super) operation: Option<String>,
    pub(super) back_hint: bool,
    pub(super) tabs: Vec<(String, bool)>,
    pub(super) loading: bool,
    pub(super) help_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProviderOperationKind {
    Start,
    Complete,
    PromptComplete,
    CancelAuth,
    Logout,
    Models,
    SelectModel,
}

/// What an in-flight [`ProviderOperationKind`] belongs to.
///
/// The model-catalog refresh spans every provider and has no real [`ProviderId`];
/// a dedicated variant keeps a fake sentinel id from colliding with a genuine
/// provider that happens to share its name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OperationScope {
    Provider(ProviderId),
    ModelCatalog,
}

/// State rendered by Ratatui. Rendering depends only on this value.
pub struct UiState {
    pub(super) composer: Composer,
    pub(super) startup_header: StartupHeader,
    transcript: Vec<TranscriptRow>,
    providers: BTreeMap<ProviderId, ProviderChoice>,
    pub(super) provider_names: BTreeMap<ProviderId, ProviderDisplayName>,
    pub(super) snapshot: CoreSnapshot,
    prompt_text: BTreeMap<u64, String>,
    response_submission: Option<SubmissionId>,
    cancelled_submissions: BTreeSet<u64>,
    submission_started_at: Option<Instant>,
    compaction_started_at: Option<Instant>,
    turn_had_tool_activity: bool,
    terminal_turn: Option<turns::TerminalTurn>,
    response_started: bool,
    quit_shortcut_expires_at: Option<Instant>,
    should_exit: bool,
    pub(super) view: Option<ActiveView>,
    pub(super) provider_operation: Option<(OperationScope, ProviderOperationKind)>,
    pub(super) provider_device_code: Option<String>,
    pub(super) activity_bar_focused: bool,
    pub(super) activity_stop_hint: String,
    pub(super) transcript_expand_hint: String,
    tool_output_expanded: bool,
    pub(super) activity_preview: Option<ActivityOutput>,
    pub(super) agent_preview: Option<AgentTranscript>,
    session_id: Option<String>,
    thinking_only: bool,
    model_picker_before_thinking: Option<ModelPicker>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            composer: Composer::default(),
            startup_header: StartupHeader::new(None, None),
            transcript: Vec::new(),
            providers: BTreeMap::new(),
            provider_names: BTreeMap::new(),
            snapshot: CoreSnapshot {
                activities: Vec::new(),
                agents: Vec::new(),
                selected_model: None,
                selected_thinking: None,
                active_submission: None,
                queued_submissions: Vec::new(),
                providers: Vec::new(),
                todos: Vec::new(),
                pending_questions: Vec::new(),
                compaction: None,
            },
            prompt_text: BTreeMap::new(),
            response_submission: None,
            cancelled_submissions: BTreeSet::new(),
            submission_started_at: None,
            compaction_started_at: None,
            turn_had_tool_activity: false,
            terminal_turn: None,
            response_started: false,
            quit_shortcut_expires_at: None,
            should_exit: false,
            view: None,
            provider_operation: None,
            provider_device_code: None,
            activity_bar_focused: false,
            activity_stop_hint: "Ctrl+X".to_owned(),
            transcript_expand_hint: "Ctrl+O".to_owned(),
            tool_output_expanded: false,
            activity_preview: None,
            agent_preview: None,
            session_id: None,
            thinking_only: false,
            model_picker_before_thinking: None,
        }
    }
}

impl UiState {
    /// Returns whether the terminal loop should exit.
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub(super) fn arm_quit_shortcut(&mut self, expires_at: Instant) {
        self.quit_shortcut_expires_at = Some(expires_at);
    }

    pub(super) fn clear_quit_shortcut(&mut self) {
        self.quit_shortcut_expires_at = None;
    }

    pub(super) fn quit_shortcut_active(&self, now: Instant) -> bool {
        self.quit_shortcut_expires_at
            .is_some_and(|expires_at| now < expires_at)
    }

    /// Returns the currently visible bottom-pane surface.
    pub fn mode(&self) -> UiMode {
        match self.view {
            None => UiMode::Input,
            Some(ActiveView::Providers(_)) => UiMode::ProviderList,
            Some(ActiveView::ProviderSettings { .. }) => UiMode::ProviderDetail,
            Some(ActiveView::AuthPrompt(_)) => UiMode::AuthPrompt,
            Some(ActiveView::Models(_)) => UiMode::ModelList,
            Some(ActiveView::Thinking(_)) => UiMode::ThinkingList,
            Some(ActiveView::Activities(_)) => UiMode::ActivityList,
            Some(ActiveView::Sessions(_)) => UiMode::SessionList,
            Some(ActiveView::AgentDiscard(_)) => UiMode::Confirmation,
            Some(ActiveView::ActivityLog(_)) => UiMode::ActivityDetail,
            Some(ActiveView::Question(_)) => UiMode::Question,
            Some(ActiveView::Context(_)) => UiMode::Context,
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
        self.snapshot.selected_model.clone()
    }

    /// Returns visible picker labels without starting provider processes.
    pub fn picker_labels(&self) -> Vec<String> {
        match &self.view {
            None => Vec::new(),
            Some(ActiveView::Providers(view)) => view.labels(),
            Some(ActiveView::ProviderSettings { actions, .. }) => {
                if let Some((_, operation)) = self.provider_operation {
                    vec![operation_label(
                        operation,
                        self.provider_device_code.as_deref(),
                    )]
                } else {
                    actions.labels()
                }
            }
            Some(ActiveView::AuthPrompt(view)) => view
                .form
                .rows()
                .into_iter()
                .map(|row| {
                    let value = row.description.unwrap_or_default();
                    format!("{}: {value}", row.label)
                })
                .collect(),
            Some(ActiveView::Models(view)) => view.labels(),
            Some(ActiveView::Thinking(view)) => view.labels(),
            Some(ActiveView::Activities(view)) => view
                .visible_rows(usize::MAX)
                .into_iter()
                .map(|row| row.label)
                .collect(),
            Some(ActiveView::Sessions(view)) => view.labels(),
            Some(ActiveView::AgentDiscard(view)) => view.labels(),
            Some(ActiveView::ActivityLog(view)) => view
                .output
                .as_ref()
                .map(activities::output_labels)
                .unwrap_or_else(|| vec!["Loading output…".to_owned()]),
            Some(ActiveView::Question(view)) => view
                .inline_presentation(usize::MAX)
                .rows
                .into_iter()
                .map(|row| row.label)
                .collect(),
            Some(ActiveView::Context(_)) => Vec::new(),
        }
    }

    /// Returns model-picker tabs and whether each tab is active.
    pub fn picker_tabs(&self) -> Vec<(String, bool)> {
        match &self.view {
            Some(ActiveView::Models(picker)) => picker.tabs(),
            Some(ActiveView::Activities(picker)) => picker.tabs(),
            _ => Vec::new(),
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

    /// Returns the number of validated images attached to the current draft.
    pub fn composer_attachment_count(&self) -> usize {
        self.composer.attachment_count()
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
    pub fn reduce(&mut self, action: &UiAction) {
        match action {
            UiAction::Noop => {}
            UiAction::HistoryPrevious => self.composer.history_previous(),
            UiAction::HistoryNext => self.composer.history_next(),
            UiAction::PickerUp => self.move_picker_up(),
            UiAction::PickerDown => self.move_picker_down(),
            UiAction::PickerTabLeft => self.move_picker_tab_left(),
            UiAction::PickerTabRight => self.move_picker_tab_right(),
            UiAction::PickerBack => self.back_from_picker(),
            UiAction::CancelAndExit => self.should_exit = true,
            UiAction::PickerConfirm
            | UiAction::ShowProviders
            | UiAction::StartAuth(_)
            | UiAction::ShowModels
            | UiAction::ShowActivities
            | UiAction::ShowSessions
            | UiAction::NewSession
            | UiAction::ResumeSession(_)
            | UiAction::ShowUsage
            | UiAction::ShowContext
            | UiAction::ShowThinking
            | UiAction::Compact(_)
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    pub(super) fn set_startup_header(
        &mut self,
        model: Option<&ModelRef>,
        directory: Option<&std::path::Path>,
    ) {
        self.startup_header = StartupHeader::new(model, directory);
    }

    pub(super) fn set_startup_notice(&mut self, notice: Option<String>) {
        self.startup_header.set_notice(notice);
    }

    pub(super) fn modal_presentation(&self, visible_rows: usize) -> Option<ModalPresentation> {
        match &self.view {
            None => None,
            Some(ActiveView::Providers(view)) => Some(list_presentation(
                view,
                visible_rows,
                self.provider_operation
                    .as_ref()
                    .map(|(_, kind)| operation_label(*kind, self.provider_device_code.as_deref())),
            )),
            Some(ActiveView::Models(view)) => Some(model_picker_presentation(view, visible_rows)),
            Some(ActiveView::Thinking(view)) => Some(list_presentation(view, visible_rows, None)),
            Some(ActiveView::Activities(view)) => Some(activities::activity_presentation(
                view,
                self.activity_preview.as_ref(),
                self.agent_preview.as_ref(),
                visible_rows,
                &self.activity_stop_hint,
            )),
            Some(ActiveView::Sessions(view)) => Some(ModalPresentation {
                title: "Resume session".to_owned(),
                rows: view.visible_rows(visible_rows),
                operation: None,
                back_hint: false,
                tabs: Vec::new(),
                loading: false,
                help_hint: None,
            }),
            Some(ActiveView::AgentDiscard(view)) => Some(ModalPresentation {
                title: "Discard agent state?".to_owned(),
                rows: view.visible_rows(visible_rows),
                operation: None,
                back_hint: false,
                tabs: Vec::new(),
                loading: false,
                help_hint: Some("enter choose  esc keep agent state".to_owned()),
            }),
            Some(ActiveView::ActivityLog(_)) => None,
            Some(ActiveView::Question(_)) => None,
            Some(ActiveView::Context(_)) => None,
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
                    operation: self.provider_operation.as_ref().map(|(_, kind)| {
                        operation_label(*kind, self.provider_device_code.as_deref())
                    }),
                    back_hint: true,
                    tabs: Vec::new(),
                    loading: false,
                    help_hint: None,
                })
            }
            Some(ActiveView::AuthPrompt(view)) => Some(ModalPresentation {
                title: format!("{} — authentication", view.display_name),
                rows: view.form.rows(),
                operation: None,
                back_hint: true,
                tabs: Vec::new(),
                loading: false,
                help_hint: Some("type value  enter validate  tab next  esc cancel".to_owned()),
            }),
        }
    }

    pub(super) fn set_provider_names(&mut self, names: BTreeMap<ProviderId, ProviderDisplayName>) {
        self.provider_names = names;
    }

    pub(super) fn status_text(&self) -> String {
        let Some(model) = &self.snapshot.selected_model else {
            return "model not selected".to_owned();
        };
        let provider = self
            .provider_names
            .get(&model.provider)
            .map(ProviderDisplayName::as_str)
            .unwrap_or(model.provider.as_str());
        let status = self.snapshot.selected_thinking.as_ref().map_or_else(
            || format!("{provider} · {}", model.model.as_str()),
            |thinking| format!("{provider} · {} · {thinking}", model.model.as_str()),
        );
        self.session_id.as_ref().map_or(status.clone(), |id| {
            let short_id: String = id.chars().take(8).collect();
            format!("{status} · session {short_id}")
        })
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.insert_filter(text),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.insert_filter(text),
            Some(ActiveView::AuthPrompt(view)) => view.form.insert(text),
            Some(ActiveView::Models(view)) => view.insert_filter(text),
            Some(ActiveView::Thinking(view)) => view.insert_filter(text),
            Some(ActiveView::Activities(view)) => view.insert_filter(text),
            Some(ActiveView::Sessions(view)) => view.insert_filter(text),
            Some(ActiveView::AgentDiscard(_)) => {}
            Some(ActiveView::ActivityLog(_)) => {}
            Some(ActiveView::Question(view)) => view.insert_text(text),
            Some(ActiveView::Context(_)) => {}
            None => {}
        }
    }

    pub(super) fn backspace_filter(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.backspace_filter(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.backspace_filter(),
            Some(ActiveView::AuthPrompt(view)) => view.form.backspace(),
            Some(ActiveView::Models(view)) => view.backspace_filter(),
            Some(ActiveView::Thinking(view)) => view.backspace_filter(),
            Some(ActiveView::Activities(view)) => view.backspace_filter(),
            Some(ActiveView::Sessions(view)) => view.backspace_filter(),
            Some(ActiveView::AgentDiscard(_)) => {}
            Some(ActiveView::ActivityLog(_)) => {}
            Some(ActiveView::Question(view)) => view.backspace(),
            Some(ActiveView::Context(_)) => {}
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

    pub(super) fn select_picker_number(&mut self, one_based: usize) -> bool {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.select_number(one_based),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.select_number(one_based),
            Some(ActiveView::AuthPrompt(_)) => false,
            Some(ActiveView::Models(view)) => view.select_number(one_based),
            Some(ActiveView::Thinking(view)) => view.select_number(one_based),
            Some(ActiveView::Activities(view)) => view.select_number(one_based),
            Some(ActiveView::Sessions(view)) => view.select_number(one_based),
            Some(ActiveView::AgentDiscard(view)) => view.select_number(one_based),
            Some(ActiveView::ActivityLog(_)) => false,
            Some(ActiveView::Question(view)) => view.select_number(one_based),
            Some(ActiveView::Context(_)) => false,
            None => false,
        }
    }

    fn move_picker_up(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_up(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_up(),
            Some(ActiveView::AuthPrompt(view)) => view.form.previous(),
            Some(ActiveView::Models(view)) => view.move_up(),
            Some(ActiveView::Thinking(view)) => view.move_up(),
            Some(ActiveView::Activities(view)) => view.move_up(),
            Some(ActiveView::Sessions(view)) => view.move_up(),
            Some(ActiveView::AgentDiscard(view)) => view.move_up(),
            Some(ActiveView::ActivityLog(view)) => view.scroll_up(1),
            Some(ActiveView::Question(view)) => view.move_up(),
            Some(ActiveView::Context(view)) => view.up(1),
            None => {}
        }
    }

    fn move_picker_down(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_down(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_down(),
            Some(ActiveView::AuthPrompt(view)) => view.form.next(),
            Some(ActiveView::Models(view)) => view.move_down(),
            Some(ActiveView::Thinking(view)) => view.move_down(),
            Some(ActiveView::Activities(view)) => view.move_down(),
            Some(ActiveView::Sessions(view)) => view.move_down(),
            Some(ActiveView::AgentDiscard(view)) => view.move_down(),
            Some(ActiveView::ActivityLog(view)) => view.scroll_down(1),
            Some(ActiveView::Question(view)) => view.move_down(),
            Some(ActiveView::Context(view)) => view.down(1),
            None => {}
        }
    }

    fn move_picker_tab_left(&mut self) {
        match &mut self.view {
            Some(ActiveView::Models(picker)) => picker.tab_left(),
            Some(ActiveView::Activities(picker)) => picker.tab_left(),
            Some(ActiveView::Question(view)) => view.tab_left(),
            _ => {}
        }
    }

    fn move_picker_tab_right(&mut self) {
        match &mut self.view {
            Some(ActiveView::Models(picker)) => picker.tab_right(),
            Some(ActiveView::Activities(picker)) => picker.tab_right(),
            Some(ActiveView::Question(view)) => view.tab_right(),
            _ => {}
        }
    }

    fn back_from_picker(&mut self) {
        match &self.view {
            Some(ActiveView::ProviderSettings { .. }) => {
                self.view = Some(ActiveView::Providers(self.provider_list()));
            }
            Some(ActiveView::AuthPrompt(view)) => {
                let provider = view.provider.clone();
                self.view = Some(provider_settings(
                    self.providers
                        .get(&provider)
                        .cloned()
                        .unwrap_or_else(|| ProviderChoice {
                            id: provider,
                            display_name: view.display_name.clone(),
                            authenticated: false,
                            credential_method: None,
                            auth_methods: Vec::new(),
                        }),
                ));
            }
            Some(ActiveView::Thinking(_)) => {
                self.view = self
                    .model_picker_before_thinking
                    .take()
                    .map(ActiveView::Models);
                self.thinking_only = false;
            }
            Some(
                ActiveView::Providers(_)
                | ActiveView::Models(_)
                | ActiveView::Sessions(_)
                | ActiveView::AgentDiscard(_),
            ) => {
                self.view = None;
                self.provider_operation = None;
                self.provider_device_code = None;
            }
            Some(ActiveView::Activities(_)) => self.view = None,
            Some(ActiveView::ActivityLog(_)) => {
                let Some(ActiveView::ActivityLog(view)) = self.view.take() else {
                    return;
                };
                let view = *view;
                self.activity_preview = view.output;
                self.agent_preview = None;
                self.view = Some(ActiveView::Activities(view.picker));
            }
            Some(ActiveView::Question(_)) => {}
            Some(ActiveView::Context(_)) => self.view = None,
            None => {}
        }
    }
}
