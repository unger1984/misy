//! Snapshot-to-view projection for the terminal client.

use super::{BrowserHandoff, TuiClient};
use crate::tui::state::ProviderChoice;
use misy_core::{CoreEvent, CoreSnapshot, ProviderAuthState, ProviderId, ProviderManifest};

pub(super) fn requires_refresh(event: &CoreEvent) -> bool {
    matches!(
        event,
        CoreEvent::AuthenticationChanged { .. }
            | CoreEvent::ModelSelected { .. }
            | CoreEvent::SubmissionStarted { .. }
            | CoreEvent::Completed { .. }
            | CoreEvent::Cancelled { .. }
            | CoreEvent::Failed { .. }
    )
}

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn refresh_snapshot(&mut self) {
        let snapshot = self.core.snapshot();
        self.state.apply_snapshot(snapshot.clone());
        self.state
            .refresh_providers(provider_choices(&self.provider_manifests, &snapshot));
    }
}

pub(super) fn provider_choices(
    manifests: &[ProviderManifest],
    snapshot: &CoreSnapshot,
) -> Vec<ProviderChoice> {
    manifests
        .iter()
        .cloned()
        .map(|provider| {
            let auth = provider_auth_state(&snapshot.providers, &provider.id);
            ProviderChoice {
                id: provider.id,
                display_name: provider.display_name,
                authenticated: auth.is_some_and(|state| state.authenticated),
                credential_method: auth.and_then(|state| state.credential_method.clone()),
                auth_methods: provider.auth_methods,
            }
        })
        .collect()
}

fn provider_auth_state<'a>(
    providers: &'a [ProviderAuthState],
    provider: &ProviderId,
) -> Option<&'a ProviderAuthState> {
    providers.iter().find(|state| state.id == *provider)
}
