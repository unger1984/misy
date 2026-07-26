//! Model-cache operations exposed by the headless core.

use super::{AvailableModels, CoreError, MisyCore, models::parse_models};
use crate::{ModelInfo, ProviderId};
use serde_json::{Value, json};

impl MisyCore {
    /// Returns model catalogs from local cache for providers with stored credentials.
    ///
    /// This method does not start provider processes or make provider requests.
    ///
    /// # Errors
    ///
    /// Returns an error only when local credential-store access fails.
    pub fn cached_available_models(&self) -> Result<AvailableModels, CoreError> {
        let authenticated_providers = self.authenticated_provider_ids()?;
        let models = self
            .inner
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

    pub(super) fn fetch_models(&self, provider: &ProviderId) -> Result<FetchedModels, CoreError> {
        let credential_epoch = self.credential_epoch(provider);
        let response = self.provider_request(provider, "models.list", json!({}))?;
        let models = parse_models(provider, &response)?;
        self.save_models_if_current(provider, credential_epoch, &models);
        Ok(FetchedModels { response, models })
    }

    fn save_models_if_current(
        &self,
        provider: &ProviderId,
        expected: super::CredentialEpoch,
        models: &[ModelInfo],
    ) {
        let cache_write = {
            let _credentials = self
                .inner
                .credential_operations
                .lock()
                .expect("credential operation mutex must not be poisoned");
            if self.credential_epoch_locked(provider).value != expected.value || !expected.present {
                return;
            }
            self.inner
                .model_cache
                .stage_save(provider, models, expected.value)
        };
        // Cache persistence is an optimization and must not turn a usable provider response
        // into a failure when local storage is unavailable.
        let _ = self.inner.model_cache.persist(cache_write);
    }

    fn authenticated_provider_ids(&self) -> Result<Vec<ProviderId>, CoreError> {
        let mut authenticated = Vec::new();
        for provider in self.providers() {
            if self.has_credentials(&provider.id)? {
                authenticated.push(provider.id);
            }
        }
        Ok(authenticated)
    }
}

pub(super) struct FetchedModels {
    pub(super) response: Value,
    pub(super) models: Vec<ModelInfo>,
}
