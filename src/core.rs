use crate::{
    Config, ConfigError, ConfigStore, CredentialError, CredentialStore, Message, MisyPaths,
    ModelId, ModelInfo, ModelRef, ProviderCatalog, ProviderDiscoveryError, ProviderError,
    ProviderHost, ProviderId, ProviderManifest, ProviderRequestId, ToolCall, ToolDispatcher,
    ToolRegistry, ToolResult,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
};

mod agent;
mod events;
use events::LosslessSubscribers;

/// A stable handle for one asynchronous agent submission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubmissionId(u64);

impl SubmissionId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A canonical history item retained by the Rust core. Provider metadata stays opaque.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub message: Message,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
    pub provider_metadata: Value,
}

/// Observable changes emitted by the headless runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum CoreEvent {
    ProviderDiscovered {
        provider: ProviderId,
    },
    AuthenticationChanged {
        provider: ProviderId,
        authenticated: bool,
    },
    ModelsListed {
        provider: ProviderId,
        models: Vec<ModelInfo>,
    },
    ModelSelected {
        model: ModelRef,
    },
    SubmissionStarted {
        submission: SubmissionId,
        model: ModelRef,
    },
    TextDelta {
        submission: SubmissionId,
        delta: String,
        provider_metadata: Value,
    },
    ToolCall {
        submission: SubmissionId,
        call: ToolCall,
        provider_metadata: Value,
    },
    ToolResult {
        submission: SubmissionId,
        result: ToolResult,
    },
    Completed {
        submission: SubmissionId,
    },
    Cancelled {
        submission: SubmissionId,
    },
    Failed {
        submission: SubmissionId,
        message: String,
    },
    Shutdown,
}

/// Errors from core configuration, provider operations, and agent lifecycle checks.
#[derive(Debug)]
pub enum CoreError {
    Config(ConfigError),
    Credentials(CredentialError),
    Discovery(ProviderDiscoveryError),
    Provider(ProviderError),
    InvalidModels(String),
    UnknownModel(ModelRef),
    NoModelSelected,
    UnknownSubmission(SubmissionId),
    Shutdown,
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "configuration error: {error}"),
            Self::Credentials(error) => write!(formatter, "credential error: {error}"),
            Self::Discovery(error) => write!(formatter, "provider discovery error: {error}"),
            Self::Provider(error) => write!(formatter, "provider error: {error}"),
            Self::InvalidModels(message) => write!(formatter, "invalid models response: {message}"),
            Self::UnknownModel(model) => {
                write!(formatter, "unknown model `{}`", model.model.as_str())
            }
            Self::NoModelSelected => formatter.write_str("no model is selected"),
            Self::UnknownSubmission(id) => write!(formatter, "unknown submission {}", id.get()),
            Self::Shutdown => formatter.write_str("misy core has shut down"),
        }
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Credentials(error) => Some(error),
            Self::Discovery(error) => Some(error),
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ConfigError> for CoreError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}
impl From<CredentialError> for CoreError {
    fn from(error: CredentialError) -> Self {
        Self::Credentials(error)
    }
}
impl From<ProviderDiscoveryError> for CoreError {
    fn from(error: ProviderDiscoveryError) -> Self {
        Self::Discovery(error)
    }
}
impl From<ProviderError> for CoreError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(match error {
            ProviderError::Remote {
                provider,
                code,
                message,
                data,
            } => ProviderError::Remote {
                provider,
                code,
                message,
                data: data.map(strip_credentials),
            },
            error => error,
        })
    }
}

fn strip_credentials(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            object.remove("credentials");
            for child in object.values_mut() {
                *child = strip_credentials(std::mem::take(child));
            }
        }
        Value::Array(values) => {
            for child in values {
                *child = strip_credentials(std::mem::take(child));
            }
        }
        _ => {}
    }
    value
}

/// Public, headless agent runtime. TUI and desktop clients consume only this contract.
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
    auth_operations: Mutex<()>,
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
    pub fn discover(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::from_catalog(paths, catalog)
    }

    /// Creates a core from an already-discovered provider catalog.
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
            auth_operations: Mutex::new(()),
            model_operations: Mutex::new(()),
            session_operation: Mutex::new(()),
            next_submission: AtomicU64::new(1),
            is_shutdown: AtomicBool::new(false),
        });
        let core = Self { inner };
        core.start_provider_event_router(host.subscribe_lossless());
        for package in core.inner.catalog.packages() {
            core.emit(CoreEvent::ProviderDiscovered {
                provider: package.manifest().id.clone(),
            });
        }
        Ok(core)
    }

    pub fn providers(&self) -> Vec<ProviderManifest> {
        self.inner
            .catalog
            .packages()
            .map(|package| package.manifest().clone())
            .collect()
    }

    pub fn selected_model(&self) -> Option<ModelRef> {
        self.inner
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone()
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        self.inner
            .history
            .lock()
            .expect("history mutex must not be poisoned")
            .clone()
    }

    /// Subscribes to bounded, non-blocking core events. Slow listeners may miss events.
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

    pub fn auth_status(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let result = self.sanitize_auth_response(self.provider_request(
            provider,
            "auth.status",
            json!({}),
        )?);
        self.emit(CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: result
                .get("authenticated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        Ok(result)
    }
    pub fn start_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        Ok(
            self.sanitize_auth_response(self.provider_request(
                provider,
                "auth.start",
                json!({}),
            )?),
        )
    }
    pub fn complete_auth(
        &self,
        provider: &ProviderId,
        completion: Value,
    ) -> Result<Value, CoreError> {
        let _operation = self
            .inner
            .auth_operations
            .lock()
            .expect("auth operation mutex must not be poisoned");
        let mut result = self.provider_request(
            provider,
            "auth.complete",
            json!({ "completion": completion }),
        )?;
        self.store_returned_credentials(provider, &mut result)?;
        self.emit(CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: true,
        });
        Ok(self.sanitize_auth_response(result))
    }
    pub fn refresh_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let _operation = self
            .inner
            .auth_operations
            .lock()
            .expect("auth operation mutex must not be poisoned");
        let mut result = self.provider_request(provider, "auth.refresh", json!({}))?;
        self.store_returned_credentials(provider, &mut result)?;
        Ok(self.sanitize_auth_response(result))
    }
    pub fn logout(&self, provider: &ProviderId) -> Result<(), CoreError> {
        let _operation = self
            .inner
            .auth_operations
            .lock()
            .expect("auth operation mutex must not be poisoned");
        self.provider_request(provider, "auth.logout", json!({}))?;
        self.inner.credential_store.remove(provider)?;
        self.emit(CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: false,
        });
        Ok(())
    }
    pub fn list_models(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, CoreError> {
        let models = self.fetch_models(provider)?;
        self.emit(CoreEvent::ModelsListed {
            provider: provider.clone(),
            models: models.clone(),
        });
        Ok(models)
    }
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
        self.inner
            .config_store
            .save(&Config::with_default_model(model.clone()))?;
        *self
            .inner
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned") = Some(model.clone());
        self.emit(CoreEvent::ModelSelected { model });
        Ok(())
    }

    fn fetch_models(&self, provider: &ProviderId) -> Result<Vec<ModelInfo>, CoreError> {
        parse_models(
            provider,
            &self.provider_request(provider, "models.list", json!({}))?,
        )
    }
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
        thread::spawn(move || core.run_submission(id, model, message, active));
        Ok(id)
    }
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
        self.emit(CoreEvent::Shutdown);
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

fn parse_models(provider: &ProviderId, response: &Value) -> Result<Vec<ModelInfo>, CoreError> {
    let values = response
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidModels("missing models array".to_owned()))?;
    values
        .iter()
        .map(|value| {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::InvalidModels("model is missing string id".to_owned()))?;
            let display_name = value
                .get("display_name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CoreError::InvalidModels("model is missing string display_name".to_owned())
                })?;
            let context_window = value
                .get("context_window")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoreError::InvalidModels("model is missing u32 context_window".to_owned())
                })?;
            Ok(ModelInfo::new(
                ModelRef::new(provider.clone(), ModelId::new(id)),
                display_name,
                context_window,
            ))
        })
        .collect()
}
