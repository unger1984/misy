//! Model catalog queries and atomic model-profile selection.

use super::{AvailableModels, CoreError, CoreEvent, MisyCore, ProviderModelError};
use crate::{ModelInfo, ModelProfile, ModelRef, ProviderId};

impl MisyCore {
    /// Fetches a provider's models and emits a [`CoreEvent::ModelsListed`] event.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider request fails or its model payload is invalid.
    pub async fn list_models(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, CoreError> {
        self.inner.state.ensure_running()?;
        let models = self.inner.state.fetch_models(provider).await?.models;
        self.emit(&CoreEvent::ModelsListed {
            provider: provider.clone(),
            models: models.clone(),
        });
        Ok(models)
    }

    /// Lists models from every provider with stored credentials.
    ///
    /// # Errors
    ///
    /// Returns an error only when local credential-store access fails.
    pub async fn available_models(&self) -> Result<AvailableModels, CoreError> {
        self.inner.state.ensure_running()?;
        let mut available = AvailableModels::default();
        for provider in self.providers().await {
            if !self.has_credentials(&provider.id).await? {
                continue;
            }
            match self.inner.state.fetch_models(&provider.id).await {
                Ok(catalog) => available.models.extend(catalog.models),
                Err(error) => available.errors.push(ProviderModelError {
                    provider: provider.id,
                    provider_display_name: provider.display_name,
                    message: error.to_string(),
                }),
            }
        }
        Ok(available)
    }

    /// Selects the provider-declared default model, falling back to its first model.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider has no valid models or persistence fails.
    pub async fn select_default_model(&self, provider: &ProviderId) -> Result<ModelRef, CoreError> {
        self.inner.state.ensure_running()?;
        let operation = self.inner.state.model_operations.lock().await;
        let catalog = self.inner.state.fetch_models(provider).await?;
        let model =
            super::models::select_catalog_default(provider, &catalog.response, &catalog.models)?;
        drop(operation);
        self.select_profile(ModelProfile::new(model.clone(), None))
            .await?;
        Ok(model)
    }

    /// Validates and persists a provider-scoped model selection.
    ///
    /// # Errors
    ///
    /// Returns an error when the model is absent from the provider catalog or persistence fails.
    pub async fn select_model(&self, model: ModelRef) -> Result<(), CoreError> {
        self.select_profile(ModelProfile::new(model, None)).await
    }

    /// Validates and atomically persists a model plus optional provider-owned reasoning level.
    ///
    /// # Errors
    ///
    /// Returns an error when the model/profile is unavailable or persistence fails.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task poisoned model-selection state.
    pub async fn select_profile(&self, mut profile: ModelProfile) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let _operation = self.inner.state.model_operations.lock().await;
        let available = self
            .inner
            .state
            .fetch_models(&profile.model.provider)
            .await?
            .models;
        let Some(info) = available
            .iter()
            .find(|available| available.model == profile.model)
        else {
            return Err(CoreError::UnknownModel(profile.model));
        };
        let capability = self
            .inner
            .state
            .catalog
            .get(&profile.model.provider)
            .is_some_and(|package| package.manifest().supports_capability("thinking", 1));
        if profile.thinking.is_none() && capability {
            profile.thinking = info
                .thinking
                .as_ref()
                .map(|metadata| metadata.default.clone());
        }
        if let Some(thinking) = profile.thinking.as_deref() {
            let supported = info
                .thinking
                .as_ref()
                .is_some_and(|metadata| metadata.levels.iter().any(|level| level.id == thinking));
            if !supported || !capability {
                return Err(CoreError::UnsupportedThinking(profile));
            }
        }
        self.compact_before_downshift(&profile, &available, info.context_window)
            .await?;
        self.persist_selected_profile(profile).await
    }

    async fn compact_before_downshift(
        &self,
        profile: &ModelProfile,
        available: &[ModelInfo],
        target_window: u32,
    ) -> Result<(), CoreError> {
        let current = self
            .inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone();
        let Some(current_model) = current.filter(|model| model != &profile.model) else {
            return Ok(());
        };
        let cached = self.inner.state.model_cache.load();
        let current_window = available
            .iter()
            .find(|candidate| candidate.model == current_model)
            .or_else(|| {
                cached
                    .iter()
                    .find(|candidate| candidate.model == current_model)
            })
            .map(|candidate| candidate.context_window);
        // An unauthenticated current catalog is intentionally not persisted. Treat an unknown
        // source window conservatively; the compactor still performs no work below the target's
        // threshold, while a true cross-provider downshift cannot bypass the old-profile step.
        if current_window.is_none_or(|window| target_window < window) {
            let old_profile = ModelProfile::new(
                current_model,
                self.inner
                    .state
                    .selected_thinking
                    .lock()
                    .expect("selected thinking mutex must not be poisoned")
                    .clone(),
            );
            self.inner
                .state
                .compact_for_model_downshift(&old_profile, target_window)
                .await?;
        }
        Ok(())
    }

    /// Validates and persists a reasoning-only change for the selected model.
    ///
    /// # Errors
    ///
    /// Returns an error when no model is selected, the level is unsupported, or persistence
    /// fails. The session records this as an additive `thinking_change`, not a model change.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task poisoned model-selection state.
    pub async fn select_thinking(&self, thinking: Option<String>) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let _operation = self.inner.state.model_operations.lock().await;
        let model = self
            .inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone()
            .ok_or(CoreError::NoModelSelected)?;
        let available = self.inner.state.fetch_models(&model.provider).await?.models;
        let Some(info) = available.iter().find(|available| available.model == model) else {
            return Err(CoreError::UnknownModel(model));
        };
        let profile = ModelProfile::new(model.clone(), thinking.clone());
        let supported = thinking.as_deref().is_none_or(|level| {
            self.inner
                .state
                .catalog
                .get(&model.provider)
                .is_some_and(|package| package.manifest().supports_capability("thinking", 1))
                && info
                    .thinking
                    .as_ref()
                    .is_some_and(|metadata| metadata.levels.iter().any(|item| item.id == level))
        });
        if !supported {
            return Err(CoreError::UnsupportedThinking(profile));
        }
        let store = self.inner.state.config_store.clone();
        let saved = thinking.clone();
        tokio::task::spawn_blocking(move || {
            let mut config = store.load()?;
            config.default_thinking = saved;
            store.save(&config)
        })
        .await
        .map_err(|error| CoreError::Runtime(error.to_string()))??;
        *self
            .inner
            .state
            .selected_thinking
            .lock()
            .expect("selected thinking mutex must not be poisoned") = thinking.clone();
        self.inner.state.persist_thinking_change(thinking);
        self.emit(&CoreEvent::ModelSelected { model });
        Ok(())
    }

    async fn persist_selected_profile(&self, profile: ModelProfile) -> Result<(), CoreError> {
        let store = self.inner.state.config_store.clone();
        let saved_profile = profile.clone();
        tokio::task::spawn_blocking(move || {
            // Read-modify-write preserves unrelated fields as the config schema grows.
            let mut config = store.load()?;
            config.default_model = Some(saved_profile.model);
            config.default_thinking = saved_profile.thinking;
            store.save(&config)
        })
        .await
        .map_err(|error| CoreError::Runtime(error.to_string()))??;
        *self
            .inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned") = Some(profile.model.clone());
        *self
            .inner
            .state
            .selected_thinking
            .lock()
            .expect("selected thinking mutex must not be poisoned") = profile.thinking.clone();
        self.inner.state.persist_model_change(profile.model.clone());
        self.emit(&CoreEvent::ModelSelected {
            model: profile.model,
        });
        Ok(())
    }
}
