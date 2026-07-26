//! Persistent, best-effort cache of provider model catalogs.

use crate::{MisyPaths, ModelId, ModelInfo, ModelRef, ProviderId, config::write_atomic};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, error::Error, fmt, fs, sync::Mutex};

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
    providers: BTreeMap<String, CachedProvider>,
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
    models: Vec<CachedModel>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedModel {
    id: String,
    display_name: String,
    context_window: u32,
}

/// Versioned model catalogs saved by the core after successful provider requests.
pub struct ModelCatalogStore {
    paths: MisyPaths,
    // This lock protects only in-memory snapshots; filesystem writes happen after it is released.
    state: Mutex<ModelCatalogState>,
}

struct ModelCatalogState {
    file: ModelCatalogFile,
    provider_epochs: BTreeMap<String, u64>,
    generation: u64,
}

pub(crate) struct PendingModelCatalogWrite {
    generation: u64,
    file: ModelCatalogFile,
}

impl ModelCatalogStore {
    /// Current model-cache schema revision.
    pub const VERSION: u32 = 1;

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
            .flat_map(|(provider, catalog)| cached_models(&ProviderId::new(provider), catalog))
            .collect()
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
    pub fn remove(&self, provider: &ProviderId) -> Result<(), ModelCatalogError> {
        let write = self.stage_remove(provider, u64::MAX);
        match write {
            Some(write) => self.persist(write),
            None => Ok(()),
        }
    }

    pub(crate) fn stage_save(
        &self,
        provider: &ProviderId,
        models: &[ModelInfo],
        credential_epoch: u64,
    ) -> PendingModelCatalogWrite {
        let mut state = self
            .state
            .lock()
            .expect("model cache state mutex must not be poisoned");
        state.file.providers.insert(
            provider.as_str().to_owned(),
            CachedProvider {
                models: models.iter().map(CachedModel::from).collect(),
            },
        );
        state
            .provider_epochs
            .insert(provider.as_str().to_owned(), credential_epoch);
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
            .get(provider.as_str())
            .is_some_and(|cached_epoch| *cached_epoch > credential_epoch)
        {
            return None;
        }
        state.file.providers.remove(provider.as_str())?;
        state.provider_epochs.remove(provider.as_str());
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
        })
        .collect()
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
        fs::write(paths.models_file(), br#"{"version":2,"providers":{}}"#)
            .expect("write incompatible cache");

        assert!(ModelCatalogStore::new(paths).load().is_empty());
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
