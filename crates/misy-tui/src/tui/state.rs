//! Deterministic state rendered by the terminal client.

mod events;
mod transcript;
mod turns;

pub use transcript::TranscriptRow;

use super::{
    action::{UiAction, UiMode},
    composer::Composer,
    list::{ListRow, ListView},
    model_picker::ModelPicker,
    presentation::{
        list_presentation, model_picker_presentation, operation_label, provider_action_rows,
        provider_settings,
    },
    startup_header::StartupHeader,
};
use misy_core::{
    CoreSnapshot, ModelRef, ProviderAuthMethod, ProviderDisplayName, ProviderId, SubmissionId,
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
    response_started: bool,
    quit_shortcut_expires_at: Option<Instant>,
    should_exit: bool,
    pub(super) view: Option<ActiveView>,
    pub(super) provider_operation: Option<(OperationScope, ProviderOperationKind)>,
    pub(super) provider_device_code: Option<String>,
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
                selected_model: None,
                active_submission: None,
                queued_submissions: Vec::new(),
                providers: Vec::new(),
            },
            prompt_text: BTreeMap::new(),
            response_submission: None,
            cancelled_submissions: BTreeSet::new(),
            submission_started_at: None,
            response_started: false,
            quit_shortcut_expires_at: None,
            should_exit: false,
            view: None,
            provider_operation: None,
            provider_device_code: None,
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
        }
    }

    /// Returns model-picker tabs and whether each tab is active.
    pub fn picker_tabs(&self) -> Vec<(String, bool)> {
        match &self.view {
            Some(ActiveView::Models(picker)) => picker.tabs(),
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
            | UiAction::ShowUsage
            | UiAction::SelectModel(_)
            | UiAction::SubmitPrompt(_) => {}
        }
    }

    pub(super) fn apply_snapshot(&mut self, snapshot: CoreSnapshot) {
        if self.snapshot.active_submission != snapshot.active_submission {
            self.submission_started_at = snapshot.active_submission.map(|_| Instant::now());
            self.response_started = false;
        }
        self.snapshot = snapshot;
    }

    pub(super) fn set_startup_header(
        &mut self,
        model: Option<&ModelRef>,
        directory: Option<&std::path::Path>,
    ) {
        self.startup_header = StartupHeader::new(model, directory);
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

    fn move_picker_tab_left(&mut self) {
        if let Some(ActiveView::Models(picker)) = &mut self.view {
            picker.tab_left();
        }
    }

    fn move_picker_tab_right(&mut self) {
        if let Some(ActiveView::Models(picker)) = &mut self.view {
            picker.tab_right();
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
                self.provider_device_code = None;
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

#[cfg(test)]
mod tests {
    use super::{ActiveView, ProviderAction, ProviderChoice, UiState};
    use crate::tui::action::UiAction;
    use misy_core::{ProviderAuthMethod, ProviderDisplayName, ProviderId};

    fn provider_choice(id: &str, display_name: &str, authenticated: bool) -> ProviderChoice {
        ProviderChoice {
            id: ProviderId::new(id),
            display_name: ProviderDisplayName::new(display_name),
            authenticated,
            credential_method: authenticated.then(|| "oauth".to_owned()),
            auth_methods: vec![ProviderAuthMethod {
                id: "oauth".to_owned(),
                display_name: "Browser OAuth".to_owned(),
            }],
        }
    }

    fn refreshed_choices() -> Vec<ProviderChoice> {
        vec![
            provider_choice("fixture", "Fixture AI", true),
            provider_choice("fixture-two", "Second AI", false),
        ]
    }

    #[test]
    fn refresh_providers_keeps_picker_filter_and_selection_while_updating_rows() {
        let mut state = UiState::default();
        state.open_providers(vec![
            provider_choice("fixture", "Fixture AI", false),
            provider_choice("fixture-two", "Second AI", false),
        ]);
        state.insert_filter("second");

        state.refresh_providers(refreshed_choices());

        let Some(ActiveView::Providers(view)) = &state.view else {
            panic!("provider picker must stay open");
        };
        assert_eq!(view.labels(), ["Second AI"]);

        for _ in 0.."second".len() {
            state.backspace_filter();
        }
        state.reduce(&UiAction::PickerDown);

        state.refresh_providers(refreshed_choices());

        let Some(ActiveView::Providers(view)) = &state.view else {
            panic!("provider picker must stay open");
        };
        assert_eq!(view.labels(), ["Fixture AI", "Second AI"]);
        assert_eq!(view.selected_value(), Some(&ProviderId::new("fixture-two")));
        let rows = view.visible_rows(8);
        assert_eq!(
            rows.first().and_then(|row| row.description.as_deref()),
            Some("✓ authenticated")
        );
        assert!(rows.last().is_some_and(|row| row.selected));
    }

    #[test]
    fn refresh_providers_updates_settings_rows_without_rebuilding_the_view() {
        let mut state = UiState::default();
        state.open_providers(vec![provider_choice("fixture", "Fixture AI", false)]);
        state.open_provider_settings(&ProviderId::new("fixture"));
        assert_eq!(
            state.selected_provider_action(),
            Some((
                ProviderId::new("fixture"),
                ProviderAction::Authorize("oauth".to_owned())
            ))
        );

        state.refresh_providers(vec![provider_choice("fixture", "Fixture AI", true)]);

        let Some(ActiveView::ProviderSettings {
            credential_method,
            actions,
            ..
        }) = &state.view
        else {
            panic!("provider settings must stay open");
        };
        assert_eq!(credential_method.as_deref(), Some("oauth"));
        assert_eq!(actions.labels(), ["Log out"]);
        assert_eq!(
            state.selected_provider_action(),
            Some((ProviderId::new("fixture"), ProviderAction::Logout))
        );
    }
}
