//! Headless orchestration of provider sessions, credentials, events, and FIFO submissions.

use crate::{
    ConfigStore, MisyPaths, ModelInfo, ModelRef, ProviderId, ProviderManifest, ProviderRequestId,
    config::CredentialStore,
    model_cache::ModelCatalogStore,
    providers::{ProviderCatalog, ProviderDeadlines, ProviderHost},
    tools::{ToolDispatcher, ToolRegistry},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};

mod agent;
mod authentication;
mod cache;
mod contracts;
mod events;
mod models;
mod queue;
mod runtime;
mod snapshot;
mod usage;
use authentication::{CredentialMethodChange, ProviderCredentialState, credential_states};
use contracts::strip_credentials;
use events::{EventSubscribers, ProviderRoutes, start_provider_event_router};
use models::select_catalog_default;
use queue::SubmissionQueue;
use runtime::RuntimeControl;

// These established names are the public core-client contract.
#[allow(clippy::module_name_repetitions)]
pub use contracts::{
    AvailableModels, CoreError, CoreEvent, HistoryEntry, ProviderModelError, SubmissionId,
};
// These public snapshot names are part of the headless-client contract.
#[allow(clippy::module_name_repetitions)]
pub use snapshot::{CoreSnapshot, ProviderAuthState};

/// Public, headless async runtime used by terminal and future non-terminal clients.
///
/// Misy owns a private runtime because provider supervision and queued work must continue when a
/// client uses a different Tokio runtime or drops an operation future. The runtime itself lives on
/// a dedicated owner thread, so dropping this value from an async context never drops a runtime.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone)]
pub struct MisyCore {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    state: Arc<CoreState>,
    runtime: RuntimeControl,
}

impl CoreInner {
    fn begin_shutdown(&self) {
        self.state.close_admission();
        self.runtime.begin_shutdown();
    }
}

pub(super) struct CoreState {
    pub(super) catalog: ProviderCatalog,
    pub(super) host: Arc<ProviderHost>,
    pub(super) config_store: ConfigStore,
    pub(super) credential_store: CredentialStore,
    pub(super) model_cache: Arc<ModelCatalogStore>,
    pub(super) selected_model: Mutex<Option<ModelRef>>,
    pub(super) history: Mutex<Vec<HistoryEntry>>,
    pub(super) dispatcher: ToolDispatcher,
    pub(super) subscribers: EventSubscribers,
    pub(super) routes: ProviderRoutes,
    pub(super) provider_gates: Mutex<BTreeMap<ProviderId, Arc<AsyncMutex<()>>>>,
    pub(super) active: Mutex<BTreeMap<u64, Arc<ActiveSubmission>>>,
    pub(super) auth_operations: Mutex<BTreeMap<ProviderId, Arc<AsyncMutex<()>>>>,
    pub(super) credential_operations: AsyncMutex<()>,
    pub(super) credential_states: Mutex<BTreeMap<ProviderId, ProviderCredentialState>>,
    pub(super) model_operations: AsyncMutex<()>,
    pub(super) submission_queue: Mutex<SubmissionQueue>,
    pub(super) next_submission: AtomicU64,
    pub(super) is_shutdown: AtomicBool,
}

pub(super) struct ActiveSubmission {
    pub(super) cancelled: AtomicBool,
    pub(super) cancellation: watch::Sender<bool>,
    pub(super) request: Mutex<Option<(ProviderId, ProviderRequestId)>>,
}

impl ActiveSubmission {
    fn new() -> Self {
        let (cancellation, _) = watch::channel(false);
        Self {
            cancelled: AtomicBool::new(false),
            cancellation,
            request: Mutex::new(None),
        }
    }

    pub(super) fn cancel(&self) -> bool {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.cancellation.send_replace(true);
        true
    }

    pub(super) fn cancellation_receiver(&self) -> watch::Receiver<bool> {
        self.cancellation.subscribe()
    }
}

impl MisyCore {
    /// Discovers bundled and user-installed providers without launching either set.
    ///
    /// # Errors
    ///
    /// Returns an error when provider discovery, configuration loading, or runtime startup fails.
    pub fn discover(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::build(paths, catalog, ProviderDeadlines::default())
    }

    /// Discovers providers like [`MisyCore::discover`] but with explicit provider wait deadlines.
    ///
    /// # Errors
    ///
    /// Returns an error when provider discovery, configuration loading, or runtime startup fails.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn discover_with_deadlines(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
        deadlines: ProviderDeadlines,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::build(paths, catalog, deadlines)
    }

    /// Creates a core from an already-discovered provider catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted configuration cannot be loaded or its runtime cannot start.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn from_catalog(paths: MisyPaths, catalog: ProviderCatalog) -> Result<Self, CoreError> {
        Self::build(paths, catalog, ProviderDeadlines::default())
    }

    /// Creates a core from a catalog with explicit provider wait deadlines.
    ///
    /// Integration tests use this to keep hung-provider scenarios fast; production clients
    /// should prefer [`MisyCore::discover`] and the default deadlines.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted configuration cannot be loaded or its runtime cannot start.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn from_catalog_with_deadlines(
        paths: MisyPaths,
        catalog: ProviderCatalog,
        deadlines: ProviderDeadlines,
    ) -> Result<Self, CoreError> {
        Self::build(paths, catalog, deadlines)
    }

    fn build(
        paths: MisyPaths,
        catalog: ProviderCatalog,
        deadlines: ProviderDeadlines,
    ) -> Result<Self, CoreError> {
        let config_store = ConfigStore::new(paths.clone());
        let config = config_store.load()?;
        let credential_store = CredentialStore::new(paths.clone());
        let credential_states = credential_states(&catalog, &credential_store);
        let (runtime, owner) = RuntimeControl::new()?;
        let state = Arc::new(CoreState {
            credential_states: Mutex::new(credential_states),
            catalog: catalog.clone(),
            host: Arc::new(ProviderHost::with_handle_and_deadlines(
                catalog,
                runtime.handle.clone(),
                deadlines,
            )),
            config_store,
            credential_store,
            model_cache: Arc::new(ModelCatalogStore::new(paths)),
            selected_model: Mutex::new(config.default_model),
            history: Mutex::new(Vec::new()),
            dispatcher: ToolDispatcher::new(ToolRegistry::new()),
            subscribers: EventSubscribers::default(),
            routes: Mutex::new(BTreeMap::new()),
            provider_gates: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            auth_operations: Mutex::new(BTreeMap::new()),
            credential_operations: AsyncMutex::new(()),
            model_operations: AsyncMutex::new(()),
            submission_queue: Mutex::new(SubmissionQueue::default()),
            next_submission: AtomicU64::new(1),
            is_shutdown: AtomicBool::new(false),
        });
        owner.start(Arc::clone(&state))?;
        start_provider_event_router(
            &runtime.handle,
            Arc::clone(&state),
            state.host.subscribe_lossless(),
        );
        let core = Self {
            inner: Arc::new(CoreInner { state, runtime }),
        };
        for package in core.inner.state.catalog.packages() {
            core.emit(&CoreEvent::ProviderDiscovered {
                provider: package.manifest().id.clone(),
            });
        }
        Ok(core)
    }

    /// Returns discovered provider manifests without launching provider subprocesses.
    pub async fn providers(&self) -> Vec<ProviderManifest> {
        self.inner
            .state
            .catalog
            .packages()
            .map(|package| package.manifest().clone())
            .collect()
    }

    /// Returns the number of provider processes that are currently healthy and running.
    pub async fn running_provider_count(&self) -> usize {
        self.inner.state.host.running_provider_count()
    }

    /// Returns the selected direct-interaction model, if one exists.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task panicked while holding the selected-model mutex.
    pub async fn selected_model(&self) -> Option<ModelRef> {
        self.inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone()
    }

    /// Returns a snapshot of the normalized in-memory conversation history.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task panicked while holding the history mutex.
    pub async fn history(&self) -> Vec<HistoryEntry> {
        self.inner
            .state
            .history
            .lock()
            .expect("history mutex must not be poisoned")
            .clone()
    }

    /// Subscribes to bounded, non-blocking core events. Slow listeners may miss events.
    pub fn subscribe(&self) -> mpsc::Receiver<CoreEvent> {
        let state = &self.inner.state;
        state.subscribers.subscribe(
            state.catalog.len().max(128),
            state
                .catalog
                .packages()
                .map(|package| CoreEvent::ProviderDiscovered {
                    provider: package.manifest().id.clone(),
                }),
        )
    }

    /// Subscribes to every core event in order for lifecycle-sensitive clients.
    pub fn subscribe_lossless(&self) -> mpsc::UnboundedReceiver<CoreEvent> {
        let state = &self.inner.state;
        state
            .subscribers
            .subscribe_lossless(state.catalog.packages().map(|package| {
                CoreEvent::ProviderDiscovered {
                    provider: package.manifest().id.clone(),
                }
            }))
    }

    /// Queries and sanitizes a provider's authentication state.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or the provider request fails.
    // Hidden from the client contract: no client calls it, while integration tests exercise the
    // `auth.status` round trip and the model-cache gate through it. Unhide when a client needs
    // on-demand status queries beyond the snapshot projection.
    #[doc(hidden)]
    pub async fn auth_status(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        self.inner.state.ensure_running()?;
        let result = self
            .inner
            .state
            .provider_request(provider, "auth.status", json!({}))
            .await?;
        let result = self.sanitize_auth_response(result);
        let authenticated = result
            .get("authenticated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // A status query carries no credential record, so the recorded credential method stays.
        self.inner.state.record_authentication(
            provider,
            authenticated,
            CredentialMethodChange::Preserve,
        );
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated,
        });
        Ok(result)
    }

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
        let _operation = self.inner.state.model_operations.lock().await;
        let catalog = self.inner.state.fetch_models(provider).await?;
        let model = select_catalog_default(provider, &catalog.response, &catalog.models)?;
        self.persist_selected_model(model.clone()).await?;
        Ok(model)
    }

    /// Validates and persists a provider-scoped model selection.
    ///
    /// # Errors
    ///
    /// Returns an error when the model is absent from the provider catalog or persistence fails.
    pub async fn select_model(&self, model: ModelRef) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let _operation = self.inner.state.model_operations.lock().await;
        if !self
            .inner
            .state
            .fetch_models(&model.provider)
            .await?
            .models
            .iter()
            .any(|available| available.model == model)
        {
            return Err(CoreError::UnknownModel(model));
        }
        self.persist_selected_model(model).await
    }

    async fn persist_selected_model(&self, model: ModelRef) -> Result<(), CoreError> {
        let store = self.inner.state.config_store.clone();
        let saved_model = model.clone();
        tokio::task::spawn_blocking(move || {
            // Read-modify-write: rebuilding a fresh `Config` would silently drop every other
            // field the file carries as soon as `Config` grows. An unreadable file fails the
            // selection instead of being clobbered; a missing file falls back to defaults.
            let mut config = store.load()?;
            config.default_model = Some(saved_model);
            store.save(&config)
        })
        .await
        .map_err(|error| CoreError::Runtime(error.to_string()))??;
        *self
            .inner
            .state
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned") = Some(model.clone());
        self.emit(&CoreEvent::ModelSelected { model });
        Ok(())
    }

    /// Requests cancellation of an active submission and forwards provider cancellation when set.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownSubmission`] when the submission is no longer active.
    pub async fn cancel(&self, submission: SubmissionId) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        self.inner.state.cancel_submission(submission).await
    }

    /// Signals shutdown and waits for provider/process cleanup to finish.
    ///
    /// # Errors
    ///
    /// Runtime ownership errors are returned when the private supervisor cannot complete.
    pub async fn shutdown(&self) -> Result<(), CoreError> {
        self.inner.begin_shutdown();
        self.inner.runtime.wait_for_shutdown().await;
        Ok(())
    }

    pub(super) fn emit(&self, event: &CoreEvent) {
        self.inner.state.subscribers.emit(event);
    }

    fn sanitize_auth_response(&self, response: Value) -> Value {
        strip_credentials(response)
    }
}

impl Drop for CoreInner {
    fn drop(&mut self) {
        self.begin_shutdown();
        self.runtime.wait_for_drop();
    }
}

impl CoreState {
    fn close_admission(&self) {
        // Queue ownership linearizes the shutdown transition with submission acceptance: a
        // submission is either enqueued before this store or rejected after it, never between.
        let _queue = self
            .submission_queue
            .lock()
            .expect("submission queue mutex must not be poisoned");
        self.is_shutdown.store(true, Ordering::Release);
    }

    pub(super) fn ensure_running(&self) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Acquire) {
            Err(CoreError::Shutdown)
        } else {
            Ok(())
        }
    }

    pub(super) async fn provider_request(
        &self,
        provider: &ProviderId,
        method: &str,
        mut params: Value,
    ) -> Result<Value, CoreError> {
        self.ensure_running()?;
        if let Some(credentials) = self.load_credentials(provider).await? {
            params["credentials"] = credentials;
        }
        Ok(self.host.request(provider, method, params).await?)
    }

    pub(super) async fn load_credentials(
        &self,
        provider: &ProviderId,
    ) -> Result<Option<Value>, CoreError> {
        let store = self.credential_store.clone();
        let provider = provider.clone();
        tokio::task::spawn_blocking(move || store.load(&provider))
            .await
            .map_err(|error| CoreError::Runtime(error.to_string()))?
            .map_err(CoreError::from)
    }

    pub(super) async fn shutdown_services(&self) {
        self.close_admission();
        self.cancel_all_submissions().await;
        // Provider shutdown is best effort; clients must still observe the Shutdown event.
        let _ = self.host.shutdown().await;
        self.subscribers.emit(&CoreEvent::Shutdown);
    }
}
