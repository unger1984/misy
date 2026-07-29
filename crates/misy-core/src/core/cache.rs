//! Model-cache operations exposed by the headless core.

use super::{
    AvailableModels, CoreError, CoreState, MisyCore, authentication::CredentialEpoch,
    models::parse_models,
};
use crate::{ModelInfo, ProviderId};
use serde_json::{Value, json};
use std::sync::Arc;

impl MisyCore {
    /// Returns model catalogs from local cache for providers with stored credentials.
    ///
    /// This method does not start provider processes or make provider requests.
    ///
    /// # Errors
    ///
    /// Returns an error only when local credential-store access fails.
    pub async fn cached_available_models(&self) -> Result<AvailableModels, CoreError> {
        self.inner.state.ensure_running()?;
        let authenticated_providers = self.authenticated_provider_ids().await?;
        let models = self
            .inner
            .state
            .model_cache
            .load()
            .into_iter()
            .filter(|model| authenticated_providers.contains(&model.model.provider))
            .collect();
        Ok(AvailableModels {
            models,
            errors: Vec::new(),
        })
    }

    async fn authenticated_provider_ids(&self) -> Result<Vec<ProviderId>, CoreError> {
        let mut authenticated = Vec::new();
        for provider in self.providers().await {
            if self.has_credentials(&provider.id).await? {
                authenticated.push(provider.id);
            }
        }
        Ok(authenticated)
    }
}

impl CoreState {
    pub(super) async fn fetch_models(
        &self,
        provider: &ProviderId,
    ) -> Result<FetchedModels, CoreError> {
        let credential_epoch = self.credential_epoch(provider);
        let response = self
            .provider_request(provider, "models.list", json!({}))
            .await?;
        let models = parse_models(provider, &response)?;
        self.save_models_if_current(provider, credential_epoch, &models)
            .await;
        Ok(FetchedModels { response, models })
    }

    async fn save_models_if_current(
        &self,
        provider: &ProviderId,
        expected: CredentialEpoch,
        models: &[ModelInfo],
    ) {
        let _credentials = self.credential_operations.lock().await;
        if self.credential_epoch(provider).value != expected.value || !expected.present {
            return;
        }
        let write = self
            .model_cache
            .stage_save(provider, models, expected.value);
        let cache = Arc::clone(&self.model_cache);
        // Cache persistence is best effort and never invalidates a usable provider response.
        let _ = tokio::task::spawn_blocking(move || cache.persist(write)).await;
    }
}

pub(super) struct FetchedModels {
    pub(super) response: Value,
    pub(super) models: Vec<ModelInfo>,
}
