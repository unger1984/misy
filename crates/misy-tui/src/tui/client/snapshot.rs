//! Snapshot-to-view projection for the terminal client.

use super::{BrowserHandoff, TuiClient};
use crate::tui::state::ProviderChoice;
use misy_core::{CoreEvent, CoreSnapshot, ProviderAuthState, ProviderId, ProviderManifest};

pub(super) fn requires_refresh(event: &CoreEvent) -> bool {
    // Exhaustive on purpose: a new `CoreEvent` variant must fail compilation
    // here so its refresh decision is made explicitly, not silently default
    // to `false` and leave a stale projection behind.
    match event {
        CoreEvent::AuthenticationChanged { .. } | CoreEvent::ModelSelected { .. } => true,
        CoreEvent::SubmissionAccepted { .. }
        | CoreEvent::SubmissionStarted { .. }
        | CoreEvent::Completed { .. }
        | CoreEvent::Cancelled { .. }
        | CoreEvent::Failed { .. } => true,
        CoreEvent::ProviderDiscovered { .. }
        | CoreEvent::ModelsListed { .. }
        | CoreEvent::TextDelta { .. }
        | CoreEvent::ToolCall { .. }
        | CoreEvent::ToolResult { .. }
        | CoreEvent::Shutdown => false,
    }
}

/// Returns whether the event can change what the provider picker shows.
///
/// Provider manifests are fixed for the client's lifetime, so only an
/// authentication change can alter the picker's rows; submission and model
/// events must not rebuild the list because that would reset typed input.
pub(super) fn affects_provider_choices(event: &CoreEvent) -> bool {
    // Same exhaustiveness requirement as `requires_refresh`: a new variant
    // must fail compilation so its effect on the picker is decided here.
    match event {
        CoreEvent::AuthenticationChanged { .. } => true,
        CoreEvent::ProviderDiscovered { .. }
        | CoreEvent::ModelsListed { .. }
        | CoreEvent::ModelSelected { .. }
        | CoreEvent::SubmissionAccepted { .. }
        | CoreEvent::SubmissionStarted { .. }
        | CoreEvent::TextDelta { .. }
        | CoreEvent::ToolCall { .. }
        | CoreEvent::ToolResult { .. }
        | CoreEvent::Completed { .. }
        | CoreEvent::Cancelled { .. }
        | CoreEvent::Failed { .. }
        | CoreEvent::Shutdown => false,
    }
}

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn refresh_core_projection(&mut self) {
        self.state.apply_snapshot(self.core.snapshot());
    }

    pub(super) fn refresh_provider_choices(&mut self) {
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
