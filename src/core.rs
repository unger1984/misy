use crate::{
    Config, ConfigStore, CredentialStore, Message, MisyPaths, ModelInfo, ModelRef, ProviderCatalog,
    ProviderHost, ProviderId, ProviderManifest, ProviderRequestId, ToolDispatcher, ToolRegistry,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

mod agent;
mod contracts;
mod events;
mod models;
mod usage;
use contracts::strip_credentials;
// These established names are the public core-client contract.
#[allow(clippy::module_name_repetitions)]
pub use contracts::{
    AvailableModels, CoreError, CoreEvent, HistoryEntry, ProviderModelError, SubmissionId,
};
use events::LosslessSubscribers;
use models::{parse_models, select_catalog_default};

/// Public, headless agent runtime. TUI and desktop clients consume only this contract.
// The established public runtime name is the crate's primary client contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone)]
pub struct MisyCore {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    catalog: ProviderCatalog,
    host: Arc<ProviderHost>,
    config_store: ConfigStore,
    credential_store: CredentialStore,
    selected_model: Mutex<Option<ModelRef>>,
    history: Mutex<Vec<HistoryEntry>>,
    dispatcher: ToolDispatcher,
    subscribers: Mutex<Vec<SyncSender<CoreEvent>>>,
    lossless_subscribers: LosslessSubscribers,
    routes: Mutex<BTreeMap<String, Sender<crate::ProviderEvent>>>,
    provider_gates: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    active: Mutex<BTreeMap<u64, Arc<ActiveSubmission>>>,
    auth_operations: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    credential_operations: Mutex<()>,
    model_operations: Mutex<()>,
    session_operation: Mutex<()>,
    next_submission: AtomicU64,
    is_shutdown: AtomicBool,
}

struct ActiveSubmission {
    cancelled: AtomicBool,
    request: Mutex<Option<(ProviderId, ProviderRequestId)>>,
}

impl MisyCore {
    /// Discovers bundled and user-installed providers without launching either set.
    ///
    /// # Errors
    ///
    /// Returns an error when provider discovery or persisted configuration loading fails.
    pub fn discover(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::from_catalog(paths, catalog)
    }

    /// Creates a core from an already-discovered provider catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted configuration cannot be loaded.
    pub fn from_catalog(paths: MisyPaths, catalog: ProviderCatalog) -> Result<Self, CoreError> {
        let config_store = ConfigStore::new(paths.clone());
        let config = config_store.load()?;
        let host = Arc::new(ProviderHost::new(catalog.clone()));
        let inner = Arc::new(CoreInner {
            catalog,
            host: Arc::clone(&host),
            config_store,
            credential_store: CredentialStore::new(paths),
            selected_model: Mutex::new(config.default_model),
            history: Mutex::new(Vec::new()),
            dispatcher: ToolDispatcher::new(ToolRegistry::new()),
            subscribers: Mutex::new(Vec::new()),
            lossless_subscribers: LosslessSubscribers::default(),
            routes: Mutex::new(BTreeMap::new()),
            provider_gates: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            auth_operations: Mutex::new(BTreeMap::new()),
            credential_operations: Mutex::new(()),
            model_operations: Mutex::new(()),
            session_operation: Mutex::new(()),
            next_submission: AtomicU64::new(1),
            is_shutdown: AtomicBool::new(false),
        });
        let core = Self { inner };
        core.start_provider_event_router(host.subscribe_lossless());
        for package in core.inner.catalog.packages() {
            core.emit(&CoreEvent::ProviderDiscovered {
                provider: package.manifest().id.clone(),
            });
        }
        Ok(core)
    }

    /// Returns discovered provider manifests without launching provider subprocesses.
    pub fn providers(&self) -> Vec<ProviderManifest> {
        self.inner
            .catalog
            .packages()
            .map(|package| package.manifest().clone())
            .collect()
    }

    /// Reports whether opaque credentials exist for `provider`.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential store cannot be read.
    pub fn has_credentials(&self, provider: &ProviderId) -> Result<bool, CoreError> {
        Ok(self.inner.credential_store.load(provider)?.is_some())
    }

    /// Returns the saved authentication method without starting the provider process.
    ///
    /// Credentials stay opaque to the core; this exposes only the provider-owned `type` label
    /// required to describe an existing login in clients.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential store cannot be read.
    pub fn credential_method(&self, provider: &ProviderId) -> Result<Option<String>, CoreError> {
        Ok(self
            .inner
            .credential_store
            .load(provider)?
            .and_then(|credentials| {
                credentials
                    .get("type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            }))
    }

    /// Returns the number of provider processes that are currently healthy and running.
    ///
    /// # Panics
    ///
    /// Panics if an internal lifecycle mutex is poisoned, which indicates a previous core thread
    /// panicked while mutating lifecycle state.
    pub fn running_provider_count(&self) -> usize {
        self.inner.host.running_provider_count()
    }

    /// Returns the selected direct-interaction model, if one exists.
    ///
    /// # Panics
    ///
    /// Panics if the selected-model mutex is poisoned by an earlier core-thread panic.
    pub fn selected_model(&self) -> Option<ModelRef> {
        self.inner
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone()
    }

    /// Returns a snapshot of the normalized in-memory conversation history.
    ///
    /// # Panics
    ///
    /// Panics if the history mutex is poisoned by an earlier core-thread panic.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.inner
            .history
            .lock()
            .expect("history mutex must not be poisoned")
            .clone()
    }

    /// Subscribes to bounded, non-blocking core events. Slow listeners may miss events.
    ///
    /// # Panics
    ///
    /// Panics if the subscriber mutex is poisoned by an earlier core-thread panic.
    pub fn subscribe(&self) -> Receiver<CoreEvent> {
        let snapshot_capacity = self.inner.catalog.len().max(128);
        let (sender, receiver) = mpsc::sync_channel(snapshot_capacity);
        for package in self.inner.catalog.packages() {
            let _ = sender.try_send(CoreEvent::ProviderDiscovered {
                provider: package.manifest().id.clone(),
            });
        }
        self.inner
            .subscribers
            .lock()
            .expect("core subscribers mutex must not be poisoned")
            .push(sender);
        receiver
    }

    /// Subscribes to every core event in order. Interactive clients use this to
    /// retain terminal lifecycle events while a provider emits a large stream.
    pub fn subscribe_lossless(&self) -> Receiver<CoreEvent> {
        self.inner
            .lossless_subscribers
            .subscribe(
                self.inner
                    .catalog
                    .packages()
                    .map(|package| CoreEvent::ProviderDiscovered {
                        provider: package.manifest().id.clone(),
                    }),
            )
    }

    /// Queries and sanitizes a provider's authentication state.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or the provider request fails.
    pub fn auth_status(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let result = self.sanitize_auth_response(self.provider_request(
            provider,
            "auth.status",
            json!({}),
        )?);
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: result
                .get("authenticated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        Ok(result)
    }
    /// Starts a provider-owned authentication flow without exposing credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or the provider cannot start authentication.
    pub fn start_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let package =
            self.inner.catalog.get(provider.as_str()).ok_or_else(|| {
                crate::ProviderError::UnknownProvider(provider.as_str().to_owned())
            })?;
        let method = package
            .manifest()
            .auth_methods
            .first()
            .map(|method| method.id.clone())
            .ok_or_else(|| CoreError::UnsupportedAuthMethod {
                provider: provider.clone(),
                method: "default".to_owned(),
            })?;
        self.start_auth_with_method(provider, &method)
    }

    /// Starts one provider-declared authentication method without exposing credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when `method` is not declared by the provider, the core is shut down, or
    /// the provider cannot start authentication.
    pub fn start_auth_with_method(
        &self,
        provider: &ProviderId,
        method: &str,
    ) -> Result<Value, CoreError> {
        let package =
            self.inner.catalog.get(provider.as_str()).ok_or_else(|| {
                crate::ProviderError::UnknownProvider(provider.as_str().to_owned())
            })?;
        if !package
            .manifest()
            .auth_methods
            .iter()
            .any(|candidate| candidate.id == method)
        {
            return Err(CoreError::UnsupportedAuthMethod {
                provider: provider.clone(),
                method: method.to_owned(),
            });
        }
        Ok(self.sanitize_auth_response(self.provider_request(
            provider,
            "auth.start",
            json!({ "method": method }),
        )?))
    }
    /// Completes a provider-owned authentication flow and persists returned credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when provider completion or private credential persistence fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    // Consuming opaque provider payloads avoids cloning potentially large authentication state.
    #[allow(clippy::needless_pass_by_value)]
    pub fn complete_auth(
        &self,
        provider: &ProviderId,
        session: Value,
        completion: Value,
    ) -> Result<Value, CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        let mut result = self.provider_request(
            provider,
            "auth.complete",
            json!({ "session": session, "completion": completion }),
        )?;
        let _credentials = self
            .inner
            .credential_operations
            .lock()
            .expect("credential operation mutex must not be poisoned");
        self.store_returned_credentials(provider, &mut result)?;
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: true,
        });
        Ok(self.sanitize_auth_response(result))
    }
    /// Refreshes provider credentials and persists the provider's returned replacement.
    ///
    /// # Errors
    ///
    /// Returns an error when provider refresh or private credential persistence fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    pub fn refresh_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.refresh_auth_locked(provider)
    }

    fn refresh_auth_locked(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let mut result = self.provider_request(provider, "auth.refresh", json!({}))?;
        let _credentials = self
            .inner
            .credential_operations
            .lock()
            .expect("credential operation mutex must not be poisoned");
        self.store_returned_credentials(provider, &mut result)?;
        Ok(self.sanitize_auth_response(result))
    }

    /// Logs out a provider and removes its stored opaque credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when logout or credential removal fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    pub fn logout(&self, provider: &ProviderId) -> Result<(), CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.provider_request(provider, "auth.logout", json!({}))?;
        let _credentials = self
            .inner
            .credential_operations
            .lock()
            .expect("credential operation mutex must not be poisoned");
        self.inner.credential_store.remove(provider)?;
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: false,
        });
        Ok(())
    }
    /// Fetches a provider's models and emits a [`CoreEvent::ModelsListed`] event.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider request fails or its model payload is invalid.
    pub fn list_models(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, CoreError> {
        let models = self.fetch_models(provider)?;
        self.emit(&CoreEvent::ModelsListed {
            provider: provider.clone(),
            models: models.clone(),
        });
        Ok(models)
    }
    /// Lists models from every provider with stored credentials.
    ///
    /// A provider failure is recorded in [`AvailableModels::errors`] so another provider can
    /// still be selected. Local credential-store failures are returned directly.
    ///
    /// # Errors
    ///
    /// Returns an error only when local credential-store access fails.
    pub fn available_models(&self) -> Result<AvailableModels, CoreError> {
        let mut available = AvailableModels::default();
        for provider in self.providers() {
            if !self.has_credentials(&provider.id)? {
                continue;
            }
            match self.fetch_models(&provider.id) {
                Ok(models) => available.models.extend(models),
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
    /// Returns an error when the provider has no valid models or the selected model cannot be
    /// persisted.
    ///
    /// # Panics
    ///
    /// Panics if the model-operation mutex is poisoned by an earlier core-thread panic.
    pub fn select_default_model(&self, provider: &ProviderId) -> Result<ModelRef, CoreError> {
        let _operation = self
            .inner
            .model_operations
            .lock()
            .expect("model operation mutex must not be poisoned");
        let response = self.provider_request(provider, "models.list", json!({}))?;
        let models = parse_models(provider, &response)?;
        let model = select_catalog_default(provider, &response, &models)?;
        self.persist_selected_model(model.clone())?;
        Ok(model)
    }
    /// Validates and persists a provider-scoped model selection.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider model list fails, does not include `model`, or the
    /// selection cannot be persisted.
    ///
    /// # Panics
    ///
    /// Panics if the model-operation mutex is poisoned by an earlier core-thread panic.
    pub fn select_model(&self, model: ModelRef) -> Result<(), CoreError> {
        let _operation = self
            .inner
            .model_operations
            .lock()
            .expect("model operation mutex must not be poisoned");
        if !self
            .fetch_models(&model.provider)?
            .iter()
            .any(|available| available.model == model)
        {
            return Err(CoreError::UnknownModel(model));
        }
        self.persist_selected_model(model)
    }

    fn fetch_models(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, CoreError> {
        parse_models(
            provider,
            &self.provider_request(provider, "models.list", json!({}))?,
        )
    }
    fn persist_selected_model(&self, model: ModelRef) -> Result<(), CoreError> {
        self.inner
            .config_store
            .save(&Config::with_default_model(model.clone()))?;
        *self
            .inner
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned") = Some(model.clone());
        self.emit(&CoreEvent::ModelSelected { model });
        Ok(())
    }
    /// Starts asynchronous processing of one user or system message.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NoModelSelected`] when no direct-interaction model is selected, or
    /// [`CoreError::Shutdown`] after shutdown.
    ///
    /// # Panics
    ///
    /// Panics if the active-submission mutex is poisoned by an earlier core-thread panic.
    pub fn submit(&self, message: Message) -> Result<SubmissionId, CoreError> {
        self.ensure_running()?;
        let model = self.selected_model().ok_or(CoreError::NoModelSelected)?;
        let id = SubmissionId(self.inner.next_submission.fetch_add(1, Ordering::Relaxed));
        let active = Arc::new(ActiveSubmission {
            cancelled: AtomicBool::new(false),
            request: Mutex::new(None),
        });
        self.inner
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .insert(id.get(), Arc::clone(&active));
        let core = self.clone();
        thread::spawn(move || core.run_submission(id, &model, message, &active));
        Ok(id)
    }
    /// Requests cancellation of an active submission and forwards provider cancellation when set.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownSubmission`] when the submission is no longer active.
    ///
    /// # Panics
    ///
    /// Panics if an active-submission mutex is poisoned by an earlier core-thread panic.
    pub fn cancel(&self, submission: SubmissionId) -> Result<(), CoreError> {
        let active = self
            .inner
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .get(&submission.get())
            .cloned()
            .ok_or(CoreError::UnknownSubmission(submission))?;
        active.cancelled.store(true, Ordering::Release);
        if let Some((provider, request)) = active
            .request
            .lock()
            .expect("active request mutex must not be poisoned")
            .clone()
        {
            let _ = self.inner.host.cancel_request(&provider, request);
        }
        Ok(())
    }
    /// Cancels active work, terminates provider processes, and emits [`CoreEvent::Shutdown`].
    ///
    /// # Errors
    ///
    /// Returns an error if provider process shutdown fails.
    ///
    /// # Panics
    ///
    /// Panics if an active-submission mutex is poisoned by an earlier core-thread panic.
    pub fn shutdown(&self) -> Result<(), CoreError> {
        if self.inner.is_shutdown.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let active: Vec<_> = self
            .inner
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .values()
            .cloned()
            .collect();
        for submission in active {
            submission.cancelled.store(true, Ordering::Release);
            if let Some((provider, request)) = submission
                .request
                .lock()
                .expect("active request mutex must not be poisoned")
                .clone()
            {
                let _ = self.inner.host.cancel_request(&provider, request);
            }
        }
        self.inner.host.shutdown()?;
        self.emit(&CoreEvent::Shutdown);
        Ok(())
    }
    fn ensure_running(&self) -> Result<(), CoreError> {
        if self.inner.is_shutdown.load(Ordering::Acquire) {
            Err(CoreError::Shutdown)
        } else {
            Ok(())
        }
    }
    fn provider_request(
        &self,
        provider: &ProviderId,
        method: &str,
        mut params: Value,
    ) -> Result<Value, CoreError> {
        self.ensure_running()?;
        if let Some(credentials) = self.inner.credential_store.load(provider)? {
            params["credentials"] = credentials;
        }
        Ok(self.inner.host.request(provider, method, params)?)
    }

    pub(super) fn refresh_expiring_credentials(
        &self,
        provider: &ProviderId,
    ) -> Result<(), CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.refresh_expiring_credentials_locked(provider)
    }

    fn refresh_expiring_credentials_locked(&self, provider: &ProviderId) -> Result<(), CoreError> {
        let credentials = self.inner.credential_store.load(provider)?;
        let Some(expires_at) = credentials
            .as_ref()
            .and_then(|value| value.get("expires_at"))
            .and_then(Value::as_u64)
        else {
            return Ok(());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| CoreError::InvalidUsage(error.to_string()))?
            .as_millis();
        if u128::from(expires_at) <= now.saturating_add(60_000) {
            self.refresh_auth_locked(provider)?;
        }
        Ok(())
    }
    fn auth_operation(&self, provider: &ProviderId) -> Arc<Mutex<()>> {
        let mut operations = self
            .inner
            .auth_operations
            .lock()
            .expect("auth operations mutex must not be poisoned");
        Arc::clone(
            operations
                .entry(provider.as_str().to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }
    fn store_returned_credentials(
        &self,
        provider: &ProviderId,
        response: &mut Value,
    ) -> Result<(), CoreError> {
        if let Some(credentials) = response.get("credentials").cloned() {
            self.inner.credential_store.save(provider, credentials)?;
            if let Some(object) = response.as_object_mut() {
                object.remove("credentials");
            }
        }
        Ok(())
    }

    fn sanitize_auth_response(&self, response: Value) -> Value {
        strip_credentials(response)
    }
}
