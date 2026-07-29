//! Provider-list state and operation identity bookkeeping.

use super::{ActiveView, OperationScope, ProviderChoice, ProviderOperationKind, UiState};
use crate::tui::{
    list::{ListRow, ListView},
    presentation::{provider_action_rows, provider_settings},
};
use misy_core::ProviderId;

impl UiState {
    pub(in crate::tui) fn open_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.replace_providers(providers);
        self.view = Some(ActiveView::Providers(self.provider_list()));
    }

    pub(in crate::tui) fn refresh_providers(&mut self, providers: Vec<ProviderChoice>) {
        self.replace_providers(providers);
        match &self.view {
            Some(ActiveView::ProviderSettings { provider, .. }) => {
                let Some(choice) = self.providers.get(provider).cloned() else {
                    return;
                };
                let rows = provider_action_rows(&choice);
                // Preserve typed filters and selection while authentication changes in the core.
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

    pub(in crate::tui) fn open_provider_settings(&mut self, provider: &ProviderId) {
        let Some(choice) = self.providers.get(provider).cloned() else {
            return;
        };
        self.view = Some(provider_settings(choice));
    }

    pub(in crate::tui) fn finish_provider_operation(
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

    pub(in crate::tui) fn set_provider_operation(
        &mut self,
        provider: ProviderId,
        kind: ProviderOperationKind,
        device_code: Option<String>,
    ) {
        self.provider_operation = Some((OperationScope::Provider(provider), kind));
        self.provider_device_code = device_code;
    }

    pub(in crate::tui) fn set_model_catalog_operation(&mut self) {
        self.provider_operation =
            Some((OperationScope::ModelCatalog, ProviderOperationKind::Models));
        self.provider_device_code = None;
    }

    pub(in crate::tui) fn finish_model_catalog_operation(&mut self) -> bool {
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

    pub(super) fn provider_list(&self) -> ListView<ProviderId> {
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
}
