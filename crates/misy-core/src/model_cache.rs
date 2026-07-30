//! Persistent, best-effort cache of provider model catalogs.

use crate::{
    InputModality, MisyPaths, ModelId, ModelInfo, ModelRef, ProviderId, ThinkingInfo,
    config::write_atomic,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const MODEL_CACHE_TTL_MS: u64 = 15 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogSource {
    Remote,
    Bundled,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedCatalog {
    pub(crate) models: Vec<ModelInfo>,
    pub(crate) fetched_at: u64,
    pub(crate) source: CatalogSource,
}

/// Errors while persisting the non-critical model catalog cache.
#[derive(Debug)]
pub enum ModelCatalogError {
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The cache could not be serialized.
    Serialize(serde_json::Error),
}

impl fmt::Display for ModelCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Serialize(error) => write!(formatter, "could not serialize model cache: {error}"),
        }
    }
}

impl Error for ModelCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Serialize(error) => Some(error),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelCatalogFile {
    version: u32,
    providers: BTreeMap<ProviderId, CachedProvider>,
}

impl Default for ModelCatalogFile {
    fn default() -> Self {
        Self {
            version: ModelCatalogStore::VERSION,
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedProvider {
    #[serde(default)]
    fetched_at: u64,
    #[serde(default = "remote_source")]
    source: CatalogSource,
    models: Vec<CachedModel>,
}

const fn remote_source() -> CatalogSource {
    CatalogSource::Remote
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedModel {
    id: String,
    display_name: String,
    context_window: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pricing: Option<String>,
    #[serde(default)]
    thinking: Option<ThinkingInfo>,
    #[serde(default = "text_only_modalities")]
    input_modalities: Vec<InputModality>,
}

/// Versioned model catalogs saved by the core after successful provider requests.
pub struct ModelCatalogStore {
    paths: MisyPaths,
    // This lock protects only in-memory snapshots; filesystem writes happen after it is released.
    state: Mutex<ModelCatalogState>,
}

struct ModelCatalogState {
    file: ModelCatalogFile,
    provider_epochs: BTreeMap<ProviderId, u64>,
    generation: u64,
}

pub(crate) struct PendingModelCatalogWrite {
    generation: u64,
    file: ModelCatalogFile,
}

impl ModelCatalogStore {
    /// Current model-cache schema revision.
    pub const VERSION: u32 = 3;

    /// Creates a model catalog store rooted at `paths`.
    pub fn new(paths: MisyPaths) -> Self {
        let file = read_file(&paths);
        Self {
            paths,
            state: Mutex::new(ModelCatalogState {
                file,
                provider_epochs: BTreeMap::new(),
                generation: 0,
            }),
        }
    }

    /// Returns all cached model metadata.
    ///
    /// Missing, unreadable, malformed, and incompatible cache files are treated as empty because
    /// the cache must never prevent a client from refreshing provider catalogs.
    ///
    /// # Panics
    ///
    /// Panics if a prior cache mutation panicked while holding the in-memory state mutex.
    pub fn load(&self) -> Vec<ModelInfo> {
        self.state
            .lock()
            .expect("model cache state mutex must not be poisoned")
            .file
            .clone()
            .providers
            .into_iter()
            .flat_map(|(provider, catalog)| cached_models(&provider, catalog))
            .collect()
    }

    pub(crate) fn catalog(&self, provider: &ProviderId) -> Option<CachedCatalog> {
        let catalog = self
            .state
            .lock()
            .expect("model cache state mutex must not be poisoned")
            .file
            .providers
            .get(provider)
            .cloned()?;
        Some(CachedCatalog {
            fetched_at: catalog.fetched_at,
            source: catalog.source,
            models: cached_models(provider, catalog),
        })
    }

    pub(crate) fn is_stale(&self, provider: &ProviderId) -> bool {
        self.catalog(provider).is_none_or(|catalog| {
            catalog.source == CatalogSource::Bundled
                || now_millis().saturating_sub(catalog.fetched_at) >= MODEL_CACHE_TTL_MS
        })
    }

    /// Replaces one provider's cached catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the updated cache cannot be serialized or written.
    ///
    /// # Panics
    ///
    /// Panics if a prior cache operation panicked while holding the operation mutex.
    // Unit-test shorthand for `stage_save` + `persist`; production writes go through the staged,
    // credential-epoch-guarded path in `CoreState::save_models_if_current`.
    #[cfg(test)]
    pub fn save(
        &self,
        provider: &ProviderId,
        models: &[ModelInfo],
    ) -> Result<(), ModelCatalogError> {
        let write = self.stage_save(provider, models, 0);
        self.persist(write)
    }

    /// Removes one provider's cached catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the updated cache cannot be serialized or written.
    ///
    /// # Panics
    ///
    /// Panics if a prior cache operation panicked while holding the operation mutex.
    // Unit-test shorthand for `stage_remove` + `persist`, mirroring `save` above.
    #[cfg(test)]
    pub fn remove(&self, provider: &ProviderId) -> Result<(), ModelCatalogError> {
        let write = self.stage_remove(provider, u64::MAX);
        match write {
            Some(write) => self.persist(write),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn stage_save(
        &self,
        provider: &ProviderId,
        models: &[ModelInfo],
        credential_epoch: u64,
    ) -> PendingModelCatalogWrite {
        self.stage_save_with_source(provider, models, credential_epoch, CatalogSource::Remote)
    }

    pub(crate) fn stage_save_with_source(
        &self,
        provider: &ProviderId,
        models: &[ModelInfo],
        credential_epoch: u64,
        source: CatalogSource,
    ) -> PendingModelCatalogWrite {
        let mut state = self
            .state
            .lock()
            .expect("model cache state mutex must not be poisoned");
        state.file.providers.insert(
            provider.clone(),
            CachedProvider {
                fetched_at: now_millis(),
                source,
                models: models.iter().map(CachedModel::from).collect(),
            },
        );
        state
            .provider_epochs
            .insert(provider.clone(), credential_epoch);
        state.generation = state.generation.wrapping_add(1);
        PendingModelCatalogWrite {
            generation: state.generation,
            file: state.file.clone(),
        }
    }

    pub(crate) fn stage_remove(
        &self,
        provider: &ProviderId,
        credential_epoch: u64,
    ) -> Option<PendingModelCatalogWrite> {
        let mut state = self
            .state
            .lock()
            .expect("model cache state mutex must not be poisoned");
        if state
            .provider_epochs
            .get(provider)
            .is_some_and(|cached_epoch| *cached_epoch > credential_epoch)
        {
            return None;
        }
        state.file.providers.remove(provider)?;
        state.provider_epochs.remove(provider);
        state.generation = state.generation.wrapping_add(1);
        Some(PendingModelCatalogWrite {
            generation: state.generation,
            file: state.file.clone(),
        })
    }

    pub(crate) fn persist(
        &self,
        mut write: PendingModelCatalogWrite,
    ) -> Result<(), ModelCatalogError> {
        loop {
            self.write_file(&write.file)?;
            let state = self
                .state
                .lock()
                .expect("model cache state mutex must not be poisoned");
            if state.generation == write.generation {
                return Ok(());
            }
            write = PendingModelCatalogWrite {
                generation: state.generation,
                file: state.file.clone(),
            };
        }
    }

    fn write_file(&self, cache: &ModelCatalogFile) -> Result<(), ModelCatalogError> {
        let contents = serde_json::to_vec_pretty(cache).map_err(ModelCatalogError::Serialize)?;
        write_atomic(&self.paths.models_file(), &contents).map_err(ModelCatalogError::Io)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn read_file(paths: &MisyPaths) -> ModelCatalogFile {
    let Ok(contents) = fs::read(paths.models_file()) else {
        return ModelCatalogFile::default();
    };
    let Ok(cache) = serde_json::from_slice::<ModelCatalogFile>(&contents) else {
        return ModelCatalogFile::default();
    };
    if cache.version != ModelCatalogStore::VERSION {
        return ModelCatalogFile::default();
    }
    cache
}

impl From<&ModelInfo> for CachedModel {
    fn from(model: &ModelInfo) -> Self {
        Self {
            id: model.model.model.as_str().to_owned(),
            display_name: model.display_name.clone(),
            context_window: model.context_window,
            description: model.description.clone(),
            pricing: model.pricing.clone(),
            thinking: model.thinking.clone(),
            input_modalities: model.input_modalities.clone(),
        }
    }
}

fn cached_models(provider: &ProviderId, catalog: CachedProvider) -> Vec<ModelInfo> {
    catalog
        .models
        .into_iter()
        .map(|model| {
            ModelInfo::new(
                ModelRef::new(provider.clone(), ModelId::new(model.id)),
                model.display_name,
                model.context_window,
            )
            .with_input_modalities(model.input_modalities)
            .with_optional_metadata(model.description, model.pricing, model.thinking)
        })
        .collect()
}

fn text_only_modalities() -> Vec<InputModality> {
    vec![InputModality::Text]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn model(provider: &str, id: &str) -> ModelInfo {
        ModelInfo::new(
            ModelRef::new(ProviderId::new(provider), ModelId::new(id)),
            "Cached model",
            4_096,
        )
    }

    #[test]
    fn saves_and_loads_model_catalogs() {
        let temporary = tempdir().expect("temporary root");
        let store = ModelCatalogStore::new(MisyPaths::from_root(temporary.path()));
        let provider = ProviderId::new("provider-a");
        let expected = vec![model("provider-a", "model-a")];

        store.save(&provider, &expected).expect("save cache");

        assert_eq!(store.load(), expected);
    }

    #[test]
    fn writes_provider_ids_as_plain_json_object_keys() {
        let temporary = tempdir().expect("temporary root");
        let paths = MisyPaths::from_root(temporary.path());
        let store = ModelCatalogStore::new(paths.clone());
        store
            .save(
                &ProviderId::new("provider-a"),
                &[model("provider-a", "model-a")],
            )
            .expect("save cache");

        let contents = fs::read(paths.models_file()).expect("read cache file");
        let json: serde_json::Value = serde_json::from_slice(&contents).expect("cache json");
        assert_eq!(json["version"], 3);
        assert!(
            json["providers"].get("provider-a").is_some(),
            "provider id must stay a plain string key: {json}"
        );
    }

    #[test]
    fn treats_malformed_cache_as_empty() {
        let temporary = tempdir().expect("temporary root");
        let paths = MisyPaths::from_root(temporary.path());
        fs::write(paths.models_file(), b"not json").expect("write malformed cache");

        assert!(ModelCatalogStore::new(paths).load().is_empty());
    }

    #[test]
    fn treats_an_incompatible_cache_version_as_empty() {
        let temporary = tempdir().expect("temporary root");
        let paths = MisyPaths::from_root(temporary.path());
        fs::write(paths.models_file(), br#"{"version":3,"providers":{}}"#)
            .expect("write incompatible cache");

        assert!(ModelCatalogStore::new(paths).load().is_empty());
    }

    #[test]
    fn treats_version_two_cache_as_empty() {
        let temporary = tempdir().expect("temporary root");
        let paths = MisyPaths::from_root(temporary.path());
        fs::create_dir_all(paths.models_file().parent().expect("models parent"))
            .expect("create models parent");
        fs::write(
            paths.models_file(),
            br#"{
                "version": 2,
                "providers": {
                    "provider-a": {"models": [{
                        "id": "model-a",
                        "display_name": "Legacy",
                        "context_window": 4096
                    }]}
                }
            }"#,
        )
        .expect("write legacy cache");

        let models = ModelCatalogStore::new(paths).load();

        assert!(models.is_empty());
    }

    #[test]
    fn removes_only_the_requested_provider_catalog() {
        let temporary = tempdir().expect("temporary root");
        let store = ModelCatalogStore::new(MisyPaths::from_root(temporary.path()));
        let provider_a = ProviderId::new("provider-a");
        let provider_b = ProviderId::new("provider-b");
        store
            .save(&provider_a, &[model("provider-a", "model-a")])
            .expect("save provider a");
        store
            .save(&provider_b, &[model("provider-b", "model-b")])
            .expect("save provider b");

        store.remove(&provider_a).expect("remove provider a");

        assert_eq!(store.load(), vec![model("provider-b", "model-b")]);
    }
}
