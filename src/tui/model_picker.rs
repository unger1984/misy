//! Cached and refreshed model-picker state, including provider tabs.

use super::list::{ListRow, ListRowDisplay, ListView};
use crate::{AvailableModels, ModelRef, ProviderId};
use std::collections::BTreeMap;

/// One filter scope in the model picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ModelPickerTab {
    /// Shows results from every provider.
    All,
    /// Shows results belonging to one provider.
    Provider(ProviderId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PickerRow {
    provider: ProviderId,
    row: ListRow<ModelRef>,
}

/// Pure state for the model popup and its provider-scoped list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ModelPicker {
    rows: Vec<PickerRow>,
    tabs: Vec<ModelPickerTab>,
    tab_labels: Vec<String>,
    active_tab: usize,
    query: String,
    list: ListView<ModelRef>,
    loading: bool,
}

impl ModelPicker {
    /// Creates a picker that remains responsive while the first model refresh is pending.
    pub(super) fn loading() -> Self {
        Self {
            rows: Vec::new(),
            tabs: vec![ModelPickerTab::All],
            tab_labels: vec!["All".to_owned()],
            active_tab: 0,
            query: String::new(),
            list: ListView::new("Select model", Vec::new()),
            loading: true,
        }
    }

    /// Creates a picker from a cached or freshly discovered catalog.
    pub(super) fn from_available(
        available: AvailableModels,
        provider_names: &BTreeMap<String, String>,
        selected_model: Option<&ModelRef>,
    ) -> Self {
        let (rows, tabs) = rows_and_tabs(available, provider_names, selected_model);
        let tab_labels = labels_for_tabs(&tabs, provider_names);
        let mut picker = Self {
            rows,
            tabs,
            tab_labels,
            active_tab: 0,
            query: String::new(),
            list: ListView::new("Select model", Vec::new()),
            loading: false,
        };
        picker.rebuild_list(None);
        picker
    }

    /// Replaces catalog contents while retaining the active tab, query, and selected model.
    pub(super) fn refresh(
        &mut self,
        available: AvailableModels,
        provider_names: &BTreeMap<String, String>,
        selected_model: Option<&ModelRef>,
    ) {
        let active_tab = self.tabs.get(self.active_tab).cloned();
        let selected = self.list.selected_value().cloned();
        let (rows, tabs) = rows_and_tabs(available, provider_names, selected_model);
        self.rows = rows;
        self.tabs = tabs;
        self.tab_labels = labels_for_tabs(&self.tabs, provider_names);
        self.active_tab = active_tab
            .as_ref()
            .and_then(|tab| self.tabs.iter().position(|candidate| candidate == tab))
            .unwrap_or(0);
        self.loading = false;
        self.rebuild_list(selected.as_ref());
    }

    /// Moves to the previous provider tab, wrapping around the tab strip.
    pub(super) fn tab_left(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        self.rebuild_list(None);
    }

    /// Moves to the next provider tab, wrapping around the tab strip.
    pub(super) fn tab_right(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
        self.rebuild_list(None);
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        self.query.push_str(text);
        self.list.insert_filter(text);
    }

    pub(super) fn backspace_filter(&mut self) {
        self.query.pop();
        self.list.backspace_filter();
    }

    pub(super) fn move_up(&mut self) {
        self.list.move_up();
    }

    pub(super) fn move_down(&mut self) {
        self.list.move_down();
    }

    pub(super) fn selected_value(&self) -> Option<&ModelRef> {
        self.list.selected_value()
    }

    pub(super) fn select_number(&mut self, number: usize) -> bool {
        self.list.select_number(number)
    }

    pub(super) fn labels(&self) -> Vec<String> {
        if self.loading {
            vec!["Loading models…".to_owned()]
        } else {
            self.list.labels()
        }
    }

    pub(super) fn tabs(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, _)| (self.tab_labels[index].clone(), index == self.active_tab))
            .collect()
    }

    pub(super) fn visible_rows(&self, visible_rows: usize) -> Vec<ListRowDisplay> {
        self.list.visible_rows(visible_rows)
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    fn rebuild_list(&mut self, selected: Option<&ModelRef>) {
        let tab = self
            .tabs
            .get(self.active_tab)
            .cloned()
            .unwrap_or(ModelPickerTab::All);
        let rows = self
            .rows
            .iter()
            .filter(|entry| belongs_to_tab(entry, &tab))
            .map(|entry| entry.row.clone())
            .collect();
        self.list = ListView::new("Select model", rows);
        self.list.set_filter(self.query.clone());
        if let Some(selected) = selected {
            self.list.select_value(selected);
        }
    }
}

fn belongs_to_tab(entry: &PickerRow, tab: &ModelPickerTab) -> bool {
    matches!(tab, ModelPickerTab::All)
        || matches!(tab, ModelPickerTab::Provider(provider) if provider == &entry.provider)
}

fn rows_and_tabs(
    available: AvailableModels,
    provider_names: &BTreeMap<String, String>,
    selected_model: Option<&ModelRef>,
) -> (Vec<PickerRow>, Vec<ModelPickerTab>) {
    let mut providers = BTreeMap::new();
    let mut rows = available
        .models
        .into_iter()
        .map(|model| {
            let provider = model.model.provider.clone();
            providers.insert(provider.as_str().to_owned(), provider.clone());
            let provider_name = provider_names
                .get(provider.as_str())
                .cloned()
                .unwrap_or_else(|| provider.as_str().to_owned());
            let label = model.model.model.as_str().to_owned();
            let row = if selected_model == Some(&model.model) {
                ListRow::current_with_search(
                    model.model,
                    label,
                    Some(provider_name),
                    model.display_name,
                )
            } else {
                ListRow::selectable_with_search(
                    model.model,
                    label,
                    Some(provider_name),
                    model.display_name,
                )
            };
            PickerRow { provider, row }
        })
        .collect::<Vec<_>>();
    rows.extend(available.errors.into_iter().map(|error| {
        providers.insert(error.provider.as_str().to_owned(), error.provider.clone());
        PickerRow {
            provider: error.provider,
            row: ListRow::informational(format!(
                "{} — error: {}",
                error.provider_display_name, error.message
            )),
        }
    }));
    let tabs = std::iter::once(ModelPickerTab::All)
        .chain(providers.into_values().map(ModelPickerTab::Provider))
        .collect();
    (rows, tabs)
}

fn labels_for_tabs(
    tabs: &[ModelPickerTab],
    provider_names: &BTreeMap<String, String>,
) -> Vec<String> {
    tabs.iter()
        .map(|tab| match tab {
            ModelPickerTab::All => "All".to_owned(),
            ModelPickerTab::Provider(provider) => provider_names
                .get(provider.as_str())
                .cloned()
                .unwrap_or_else(|| provider.as_str().to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::ModelPicker;
    use crate::{AvailableModels, ModelId, ModelInfo, ModelRef, ProviderId};
    use std::collections::BTreeMap;

    fn available() -> AvailableModels {
        AvailableModels {
            models: vec![
                ModelInfo::new(
                    ModelRef::new(ProviderId::new("first"), ModelId::new("alpha")),
                    "Alpha",
                    1,
                ),
                ModelInfo::new(
                    ModelRef::new(ProviderId::new("second"), ModelId::new("beta")),
                    "Beta",
                    1,
                ),
            ],
            errors: Vec::new(),
        }
    }

    #[test]
    fn tabs_filter_rows_and_keep_the_query() {
        let mut picker = ModelPicker::from_available(available(), &BTreeMap::new(), None);
        picker.insert_filter("a");
        picker.tab_right();
        assert_eq!(picker.labels(), ["alpha"]);
        picker.tab_right();
        assert_eq!(picker.labels(), ["beta"]);
        assert_eq!(picker.tabs()[2], ("second".to_owned(), true));
    }
}
