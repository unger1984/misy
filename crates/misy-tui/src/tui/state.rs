//! Deterministic state rendered by the terminal client.

mod activities;
mod events;
mod sessions;
#[cfg(test)]
mod tests;
mod transcript;
mod turns;

pub(super) use activities::output_content_labels;
pub use transcript::TranscriptRow;

use super::{
    action::{UiAction, UiMode},
    activity_picker::ActivityPicker,
    composer::Composer,
    list::{ListRow, ListView},
    model_picker::ModelPicker,
    presentation::{
        list_presentation, model_picker_presentation, operation_label, provider_action_rows,
        provider_settings,
    },
    session_picker::SessionPicker,
    startup_header::StartupHeader,
};
use misy_core::{
    ActivityOutput, CoreSnapshot, ModelRef, ProviderAuthMethod, ProviderDisplayName, ProviderId,
    SubmissionId,
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
    Models(ModelPicker),
    Activities(ActivityPicker),
    Sessions(SessionPicker),
    ActivityLog(activities::ActivityLogView),
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
    session_id: Option<String>,
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
                selected_model: None,
                active_submission: None,
                queued_submissions: Vec::new(),
                providers: Vec::new(),
            },
            prompt_text: BTreeMap::new(),
            response_submission: None,
            cancelled_submissions: BTreeSet::new(),
            submission_started_at: None,
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
            session_id: None,
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
            Some(ActiveView::Models(_)) => UiMode::ModelList,
            Some(ActiveView::Activities(_)) => UiMode::ActivityList,
            Some(ActiveView::Sessions(_)) => UiMode::SessionList,
            Some(ActiveView::ActivityLog(_)) => UiMode::ActivityDetail,
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
            Some(ActiveView::Models(view)) => view.labels(),
            Some(ActiveView::Activities(view)) => view
                .visible_rows(usize::MAX)
                .into_iter()
                .map(|row| row.label)
                .collect(),
            Some(ActiveView::Sessions(view)) => view.labels(),
            Some(ActiveView::ActivityLog(view)) => view
                .output
                .as_ref()
                .map(activities::output_labels)
                .unwrap_or_else(|| vec!["Loading output…".to_owned()]),
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
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    pub(super) fn apply_snapshot(&mut self, snapshot: CoreSnapshot) {
        self.apply_snapshot_at(snapshot, Instant::now());
    }

    fn apply_snapshot_at(&mut self, snapshot: CoreSnapshot, now: Instant) {
        if self.snapshot.active_submission != snapshot.active_submission {
            self.capture_turn_transition(snapshot.active_submission, now);
        }
        let activities = snapshot.activities.clone();
        self.snapshot = snapshot;
        if !self.activity_bar_visible() {
            self.activity_bar_focused = false;
        }
        match &mut self.view {
            Some(ActiveView::Activities(picker)) => picker.refresh(activities),
            Some(ActiveView::ActivityLog(view)) => view.picker.refresh(activities),
            _ => {}
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

    pub(super) fn open_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.replace_providers(providers);
        self.view = Some(ActiveView::Providers(self.provider_list()));
    }

    pub(super) fn refresh_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.replace_providers(providers);
        match &self.view {
            Some(ActiveView::ProviderSettings { provider, .. }) => {
                let Some(choice) = self.providers.get(provider).cloned() else {
                    return;
                };
                let rows = provider_action_rows(&choice);
                // Rows are swapped into the live views instead of rebuilding
                // them so a background refresh keeps the user's filter and
                // highlight (review finding #10).
                if let Some(ActiveView::ProviderSettings {
                    display_name,
                    credential_method,
                    actions,
                    ..
                }) = &mut self.view
                {
                    *display_name = choice.display_name;
                    *credential_method = choice.credential_method;
                    actions.replace_rows(rows);
                }
            }
            Some(ActiveView::Providers(_)) => {
                let rows = self.provider_rows();
                if let Some(ActiveView::Providers(view)) = &mut self.view {
                    view.replace_rows(rows);
                }
            }
            _ => {}
        }
    }

    fn replace_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.providers = providers
            .into_iter()
            .map(|provider| (provider.id.clone(), provider))
            .collect();
    }

    pub(super) fn open_provider_settings(&mut self, provider: &ProviderId) {
        let Some(choice) = self.providers.get(provider).cloned() else {
            return;
        };
        self.view = Some(provider_settings(choice));
    }

    pub(super) fn finish_provider_operation(
        &mut self,
        provider: &ProviderId,
        kind: ProviderOperationKind,
    ) -> bool {
        let scope = OperationScope::Provider(provider.clone());
        if self.provider_operation.as_ref() == Some(&(scope, kind)) {
            self.provider_operation = None;
            self.provider_device_code = None;
            true
        } else {
            false
        }
    }

    pub(super) fn set_provider_operation(
        &mut self,
        provider: ProviderId,
        kind: ProviderOperationKind,
        device_code: Option<String>,
    ) {
        self.provider_operation = Some((OperationScope::Provider(provider), kind));
        self.provider_device_code = device_code;
    }

    pub(super) fn set_model_catalog_operation(&mut self) {
        self.provider_operation =
            Some((OperationScope::ModelCatalog, ProviderOperationKind::Models));
        self.provider_device_code = None;
    }

    pub(super) fn finish_model_catalog_operation(&mut self) -> bool {
        if matches!(
            self.provider_operation,
            Some((OperationScope::ModelCatalog, ProviderOperationKind::Models))
        ) {
            self.provider_operation = None;
            self.provider_device_code = None;
            true
        } else {
            false
        }
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
            Some(ActiveView::Activities(view)) => Some(activities::activity_presentation(
                view,
                self.activity_preview.as_ref(),
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
            Some(ActiveView::ActivityLog(_)) => None,
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
        let status = format!("{provider} · {}", model.model.as_str());
        self.session_id.as_ref().map_or(status.clone(), |id| {
            let short_id: String = id.chars().take(8).collect();
            format!("{status} · session {short_id}")
        })
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

    fn provider_list(&self) -> ListView<ProviderId> {
        ListView::new("Providers", self.provider_rows())
    }

    fn provider_rows(&self) -> Vec<ListRow<ProviderId>> {
        self.providers
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
            .collect()
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.insert_filter(text),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.insert_filter(text),
            Some(ActiveView::Models(view)) => view.insert_filter(text),
            Some(ActiveView::Activities(view)) => view.insert_filter(text),
            Some(ActiveView::Sessions(view)) => view.insert_filter(text),
            Some(ActiveView::ActivityLog(_)) => {}
            None => {}
        }
    }

    pub(super) fn backspace_filter(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.backspace_filter(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.backspace_filter(),
            Some(ActiveView::Models(view)) => view.backspace_filter(),
            Some(ActiveView::Activities(view)) => view.backspace_filter(),
            Some(ActiveView::Sessions(view)) => view.backspace_filter(),
            Some(ActiveView::ActivityLog(_)) => {}
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
            Some(ActiveView::Activities(view)) => view.select_number(one_based),
            Some(ActiveView::Sessions(view)) => view.select_number(one_based),
            Some(ActiveView::ActivityLog(_)) => false,
            None => false,
        }
    }

    fn move_picker_up(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_up(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_up(),
            Some(ActiveView::Models(view)) => view.move_up(),
            Some(ActiveView::Activities(view)) => view.move_up(),
            Some(ActiveView::Sessions(view)) => view.move_up(),
            Some(ActiveView::ActivityLog(view)) => view.scroll_up(1),
            None => {}
        }
    }

    fn move_picker_down(&mut self) {
        match &mut self.view {
            Some(ActiveView::Providers(view)) => view.move_down(),
            Some(ActiveView::ProviderSettings { actions, .. }) => actions.move_down(),
            Some(ActiveView::Models(view)) => view.move_down(),
            Some(ActiveView::Activities(view)) => view.move_down(),
            Some(ActiveView::Sessions(view)) => view.move_down(),
            Some(ActiveView::ActivityLog(view)) => view.scroll_down(1),
            None => {}
        }
    }

    fn move_picker_tab_left(&mut self) {
        match &mut self.view {
            Some(ActiveView::Models(picker)) => picker.tab_left(),
            Some(ActiveView::Activities(picker)) => picker.tab_left(),
            _ => {}
        }
    }

    fn move_picker_tab_right(&mut self) {
        match &mut self.view {
            Some(ActiveView::Models(picker)) => picker.tab_right(),
            Some(ActiveView::Activities(picker)) => picker.tab_right(),
            _ => {}
        }
    }

    fn back_from_picker(&mut self) {
        match &self.view {
            Some(ActiveView::ProviderSettings { .. }) => {
                self.view = Some(ActiveView::Providers(self.provider_list()));
            }
            Some(ActiveView::Providers(_) | ActiveView::Models(_) | ActiveView::Sessions(_)) => {
                self.view = None;
                self.provider_operation = None;
                self.provider_device_code = None;
            }
            Some(ActiveView::Activities(_)) => self.view = None,
            Some(ActiveView::ActivityLog(_)) => {
                let Some(ActiveView::ActivityLog(view)) = self.view.take() else {
                    return;
                };
                self.activity_preview = view.output;
                self.view = Some(ActiveView::Activities(view.picker));
            }
            None => {}
        }
    }
}

pub(super) fn spinner_frame(ticks: u128) -> &'static str {
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
