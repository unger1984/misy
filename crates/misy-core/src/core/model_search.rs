//! Deterministic model-facing catalog search with independent availability and freshness.

use super::{
    CoreError, CoreState, MisyCore,
    roles::{ModelAvailability, ModelFreshness, ModelSearchMatch},
};
use crate::{ModelInfo, ModelProfile, ProviderId, model_cache::CatalogSource};

const MAX_RESULTS: usize = 20;

impl MisyCore {
    /// Searches bounded model metadata, refreshing relevant stale authenticated providers.
    ///
    /// # Errors
    ///
    /// Returns an error only when local credentials cannot be read. Individual provider refresh
    /// failures retain stale results and become result warnings.
    pub async fn model_search(
        &self,
        query: Option<&str>,
        provider: Option<&ProviderId>,
        agent_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ModelSearchMatch>, CoreError> {
        self.inner
            .state
            .search_models(query, provider, agent_type, limit)
            .await
    }
}

impl CoreState {
    pub(super) async fn search_models(
        &self,
        query: Option<&str>,
        provider: Option<&ProviderId>,
        agent_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ModelSearchMatch>, CoreError> {
        self.ensure_running()?;
        let raw_query = query.unwrap_or_default().trim();
        let requested_profile = raw_query.parse::<ModelProfile>().ok();
        let query = raw_query.to_ascii_lowercase();
        let roles = self.discover_roles();
        let role_profiles = agent_type
            .and_then(|name| roles.get(name))
            .and_then(|role| role.models.as_deref());
        let refresh_warnings = refresh_relevant_catalogs(
            self,
            &query,
            provider,
            requested_profile.as_ref(),
            role_profiles,
        )
        .await;
        let mut matches = Vec::new();
        for model in self.model_cache.load() {
            if provider.is_some_and(|filter| filter != &model.model.provider) {
                continue;
            }
            let searchable = format!(
                "{} {} {}",
                model.model.provider.as_str(),
                model.model.model.as_str(),
                model.display_name
            )
            .to_ascii_lowercase();
            if requested_profile
                .as_ref()
                .is_some_and(|profile| profile.model != model.model)
                || (requested_profile.is_none()
                    && !query.is_empty()
                    && !searchable.contains(&query))
            {
                continue;
            }
            let package = self.catalog.get(&model.model.provider);
            let authenticated = self.credential_epoch(&model.model.provider).present;
            let catalog = self.model_cache.catalog(&model.model.provider);
            let freshness = match catalog.as_ref().map(|catalog| catalog.source) {
                Some(CatalogSource::Bundled) => ModelFreshness::Bundled,
                Some(CatalogSource::Remote) if self.model_cache.is_stale(&model.model.provider) => {
                    ModelFreshness::Stale
                }
                _ => ModelFreshness::Fresh,
            };
            let mut warnings = Vec::new();
            if let Some(warning) = refresh_warnings.get(&model.model.provider) {
                warnings.push(warning.clone());
            }
            let profiles = relevant_profiles(&model, requested_profile.as_ref(), role_profiles);
            for profile in profiles {
                let availability = profile_availability(
                    &model,
                    &profile,
                    package.is_some(),
                    package.is_some_and(|package| {
                        package.manifest().supports_capability("thinking", 1)
                    }),
                    authenticated,
                );
                matches.push(ModelSearchMatch {
                    selector: profile.selector(),
                    display_name: model.display_name.clone(),
                    context_window: model.context_window,
                    description: model.description.clone(),
                    pricing: model.pricing.clone(),
                    thinking_default: model
                        .thinking
                        .as_ref()
                        .map(|thinking| thinking.default.clone()),
                    thinking_levels: model.thinking.as_ref().map_or_else(Vec::new, |thinking| {
                        thinking
                            .levels
                            .iter()
                            .map(|level| level.id.clone())
                            .collect()
                    }),
                    in_role: role_profiles.is_some_and(|profiles| profiles.contains(&profile)),
                    availability,
                    freshness,
                    warnings: warnings.clone(),
                });
            }
        }
        matches.sort_by_key(|entry| {
            (
                entry.availability != ModelAvailability::Runnable,
                entry.selector.clone(),
            )
        });
        matches.truncate(limit.clamp(1, MAX_RESULTS));
        Ok(matches)
    }
}

async fn refresh_relevant_catalogs(
    core: &CoreState,
    query: &str,
    provider: Option<&ProviderId>,
    requested: Option<&ModelProfile>,
    role_profiles: Option<&[ModelProfile]>,
) -> std::collections::BTreeMap<ProviderId, String> {
    let mut warnings = std::collections::BTreeMap::new();
    for package in core.catalog.packages() {
        let manifest = package.manifest();
        if provider.is_some_and(|filter| filter != &manifest.id) {
            continue;
        }
        let relevant = query.is_empty()
            || manifest.id.as_str().to_ascii_lowercase().contains(query)
            || requested.is_some_and(|profile| profile.model.provider == manifest.id)
            || role_profiles.is_some_and(|profiles| {
                profiles
                    .iter()
                    .any(|profile| profile.model.provider == manifest.id)
            });
        if relevant
            && core.credential_epoch(&manifest.id).present
            && core.model_cache.is_stale(&manifest.id)
            && let Err(error) = core.refresh_models_if_stale(&manifest.id).await
        {
            warnings.insert(manifest.id.clone(), error.to_string());
        }
    }
    warnings
}

fn relevant_profiles(
    model: &ModelInfo,
    requested: Option<&ModelProfile>,
    role_profiles: Option<&[ModelProfile]>,
) -> Vec<ModelProfile> {
    if let Some(requested) = requested.filter(|profile| profile.model == model.model) {
        return vec![requested.clone()];
    }
    let role_matches = role_profiles
        .into_iter()
        .flatten()
        .filter(|profile| profile.model == model.model)
        .cloned()
        .collect::<Vec<_>>();
    if role_matches.is_empty() {
        vec![ModelProfile::new(model.model.clone(), None)]
    } else {
        role_matches
    }
}

fn profile_availability(
    model: &ModelInfo,
    profile: &ModelProfile,
    provider_available: bool,
    thinking_capability: bool,
    authenticated: bool,
) -> ModelAvailability {
    if !provider_available {
        return ModelAvailability::ProviderUnavailable;
    }
    let thinking_supported = profile.thinking.as_deref().is_none_or(|requested| {
        thinking_capability
            && model
                .thinking
                .as_ref()
                .is_some_and(|thinking| thinking.levels.iter().any(|level| level.id == requested))
    });
    if !thinking_supported {
        ModelAvailability::UnsupportedThinking
    } else if !authenticated {
        ModelAvailability::AuthRequired
    } else {
        ModelAvailability::Runnable
    }
}

#[cfg(test)]
mod tests {
    use super::profile_availability;
    use crate::{
        ModelAvailability, ModelId, ModelInfo, ModelProfile, ModelRef, ProviderId, ThinkingInfo,
        ThinkingLevel,
    };

    fn model() -> ModelInfo {
        ModelInfo::new(
            ModelRef::new(ProviderId::new("fixture"), ModelId::new("model")),
            "Model",
            100_000,
        )
        .with_optional_metadata(
            None,
            None,
            Some(ThinkingInfo {
                default: "medium".to_owned(),
                levels: vec![ThinkingLevel {
                    id: "medium".to_owned(),
                    description: "Balanced".to_owned(),
                }],
            }),
        )
    }

    #[test]
    fn unsupported_thinking_does_not_depend_on_cache_freshness_or_authentication() {
        let model = model();
        let profile = ModelProfile::new(model.model.clone(), Some("ultra".to_owned()));
        assert_eq!(
            profile_availability(&model, &profile, true, true, false),
            ModelAvailability::UnsupportedThinking
        );
    }
}
