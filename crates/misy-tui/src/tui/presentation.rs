//! Modal-list presentation shared by state queries and rendering.

use super::{
    list::{ListRow, ListView},
    model_picker::ModelPicker,
    state::{ActiveView, ModalPresentation, ProviderAction, ProviderChoice, ProviderOperationKind},
};

pub(super) fn operation_label(kind: ProviderOperationKind, device_code: Option<&str>) -> String {
    match (kind, device_code) {
        (ProviderOperationKind::Complete, Some(code)) => format!("Code: {code} · waiting…"),
        (ProviderOperationKind::Start, _) => "Opening browser…".to_owned(),
        (ProviderOperationKind::Complete, None) => "Waiting for browser…".to_owned(),
        (ProviderOperationKind::Logout, _) => "Logging out…".to_owned(),
        (ProviderOperationKind::Models, _) => "Loading models…".to_owned(),
        (ProviderOperationKind::SelectModel, _) => "Selecting model…".to_owned(),
    }
}

pub(super) fn list_presentation<T>(
    view: &ListView<T>,
    visible_rows: usize,
    operation: Option<String>,
) -> ModalPresentation {
    ModalPresentation {
        title: view.title().to_owned(),
        rows: view.visible_rows(visible_rows),
        operation,
        back_hint: false,
        tabs: Vec::new(),
        loading: false,
    }
}

pub(super) fn model_picker_presentation(
    picker: &ModelPicker,
    visible_rows: usize,
) -> ModalPresentation {
    ModalPresentation {
        title: "Select model".to_owned(),
        rows: picker.visible_rows(visible_rows),
        operation: None,
        back_hint: false,
        tabs: picker.tabs(),
        loading: picker.is_loading(),
    }
}

pub(super) fn provider_settings(provider: ProviderChoice) -> ActiveView {
    let rows = provider_action_rows(&provider);
    ActiveView::ProviderSettings {
        provider: provider.id,
        display_name: provider.display_name,
        credential_method: provider.credential_method,
        actions: ListView::new("Provider settings", rows),
    }
}

pub(super) fn provider_action_rows(provider: &ProviderChoice) -> Vec<ListRow<ProviderAction>> {
    if provider.authenticated {
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
    }
}
