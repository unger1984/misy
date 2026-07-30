//! Model and provider-owned thinking picker transitions.

use super::{ActiveView, UiState};
use crate::tui::list::{ListRow, ListView};
use misy_core::{ModelInfo, ModelProfile, ModelRef};

impl UiState {
    pub(in crate::tui) fn selected_model_choice(&self) -> Option<ModelRef> {
        match &self.view {
            Some(ActiveView::Models(view)) => view.selected_value().cloned(),
            _ => None,
        }
    }

    pub(in crate::tui) fn selected_profile_choice(&self) -> Option<ModelProfile> {
        match &self.view {
            Some(ActiveView::Thinking(view)) => view.selected_value().cloned(),
            _ => None,
        }
    }

    pub(in crate::tui) fn thinking_selection_is_reasoning_only(&self) -> bool {
        self.thinking_only
    }

    pub(in crate::tui) fn open_thinking(&mut self, model: &ModelInfo, selected: Option<&str>) {
        self.thinking_only = false;
        self.model_picker_before_thinking = match self.view.take() {
            Some(ActiveView::Models(picker)) => Some(picker),
            view => {
                self.view = view;
                None
            }
        };
        self.open_thinking_picker(model, selected);
    }

    pub(in crate::tui) fn open_thinking_only(&mut self, model: &ModelInfo, selected: Option<&str>) {
        self.thinking_only = true;
        self.model_picker_before_thinking = None;
        self.open_thinking_picker(model, selected);
    }

    fn open_thinking_picker(&mut self, model: &ModelInfo, selected: Option<&str>) {
        let rows = model
            .thinking
            .as_ref()
            .map(|thinking| {
                thinking
                    .levels
                    .iter()
                    .map(|level| thinking_row(model, thinking.default.as_str(), level, selected))
                    .collect()
            })
            .unwrap_or_default();
        self.view = Some(ActiveView::Thinking(ListView::new(
            format!("Thinking for {}", model.display_name),
            rows,
        )));
    }
}

fn thinking_row(
    model: &ModelInfo,
    default: &str,
    level: &misy_core::ThinkingLevel,
    selected: Option<&str>,
) -> ListRow<ModelProfile> {
    let profile = ModelProfile::new(model.model.clone(), Some(level.id.clone()));
    let label = if level.id == default {
        format!("{} (default)", level.id)
    } else {
        level.id.clone()
    };
    if selected == Some(level.id.as_str()) {
        ListRow::current(profile, label, Some(level.description.clone()))
    } else {
        ListRow::selectable(profile, label, Some(level.description.clone()))
    }
}
