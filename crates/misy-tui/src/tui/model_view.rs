//! Model-popup transitions that connect pure picker state to the TUI view state.

use super::{
    model_picker::ModelPicker,
    state::{ActiveView, UiState},
};
use misy_core::{AvailableModels, ProviderDisplayName, ProviderId};
use std::collections::BTreeMap;

impl UiState {
    pub(super) fn open_models(&mut self, available: AvailableModels) {
        self.set_model_catalog_operation();
        let picker = if available.models.is_empty() && available.errors.is_empty() {
            ModelPicker::loading()
        } else {
            ModelPicker::from_available(
                available,
                &self.provider_names,
                self.snapshot.selected_model.as_ref(),
            )
        };
        self.view = Some(ActiveView::Models(picker));
    }

    pub(super) fn finish_models(
        &mut self,
        available: AvailableModels,
        names: &BTreeMap<ProviderId, ProviderDisplayName>,
    ) {
        if !matches!(self.view, Some(ActiveView::Models(_)))
            || !self.finish_model_catalog_operation()
        {
            return;
        }
        if let Some(ActiveView::Models(picker)) = &mut self.view {
            picker.refresh(available, names, self.snapshot.selected_model.as_ref());
        }
    }
}
