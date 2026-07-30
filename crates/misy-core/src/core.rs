//! Headless orchestration of provider sessions, credentials, events, and FIFO submissions.

use crate::{
    ActivityId, ActivityOutput, CompactionConfig, ConfigStore, MisyPaths, ModelProfile, ModelRef,
    ProviderId, ProviderManifest, ProviderRequestId,
    config::CredentialStore,
    model_cache::ModelCatalogStore,
    providers::{ProviderCatalog, ProviderDeadlines, ProviderHost},
    tools::{ActivityEvent, ToolDispatcher, ToolRegistry},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};

mod agent;
pub(crate) mod agents;
mod authentication;
mod cache;
mod compaction;
mod context_report;
mod contracts;
mod events;
mod images;
mod instruction_contracts;
mod instruction_paths;
mod instructions;
mod model_search;
mod model_selection;
mod models;
mod options;
pub(crate) mod questions;
mod queue;
mod roles;
mod runtime;
mod session;
mod session_api;
mod snapshot;
pub(crate) mod todos;
mod tool_router;
pub(crate) mod turn;
mod usage;
use agents::AgentRegistry;
use authentication::{CredentialMethodChange, ProviderCredentialState, credential_states};
use contracts::strip_credentials;
use events::{EventSubscribers, start_provider_event_router};
use queue::SubmissionQueue;
use runtime::RuntimeControl;

// These established names are the public core-client contract.
pub use agents::{
    AgentAttempt, AgentId, AgentSummary, AgentTranscript, AgentTranscriptEntry,
    AgentTranscriptEntryKind,
};
#[allow(clippy::module_name_repetitions)]
pub use contracts::{
    AvailableModels, CoreError, CoreEvent, HistoryEntry, ProviderModelError, SubmissionId,
};
pub use instruction_contracts::{
    ContextCategory, ContextCategoryUsage, ContextReport, ContextReportState, InstructionOwner,
    InstructionScope, InstructionSourceKind, InstructionSourceStatus, InstructionSourceSummary,
    InstructionWarning, InstructionWarningReason,
};
pub use questions::{
    ClientCapabilities, CoreOptions, QuestionItem, QuestionOption, QuestionRequest,
    QuestionRequestId, QuestionResponse, QuestionSource,
};
pub use roles::{
    AgentRoleSource, AgentRoleStatus, AgentRoleSummary, ModelAvailability, ModelFreshness,
    ModelSearchMatch,
};
pub use session::{
    CompactionActivity, CompactionCheckpoint, ResumeOutcome, SessionError, SessionSummary,
};
pub use todos::{TodoItem, TodoStatus};
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
    self_reference: OnceLock<Weak<CoreState>>,
    pub(super) catalog: ProviderCatalog,
    pub(super) host: Arc<ProviderHost>,
    pub(super) config_store: ConfigStore,
    pub(super) credential_store: CredentialStore,
    pub(super) model_cache: Arc<ModelCatalogStore>,
    pub(super) misy_paths: MisyPaths,
    pub(super) instructions: Mutex<instructions::InstructionRuntime>,
    pub(super) selected_model: Mutex<Option<ModelRef>>,
    pub(super) selected_thinking: Mutex<Option<String>>,
    pub(super) compaction_config: CompactionConfig,
    pub(super) compaction: Mutex<Option<(CompactionActivity, Arc<ActiveSubmission>)>>,
    pub(super) keybindings: BTreeMap<String, Vec<String>>,
    pub(super) client_capabilities: ClientCapabilities,
    pub(super) history: Arc<Mutex<Vec<HistoryEntry>>>,
    pub(super) active_history: Arc<Mutex<Vec<HistoryEntry>>>,
    /// Root checklist uses an independent short-lived lock for snapshot projection.
    pub(super) todos: Arc<Mutex<Vec<TodoItem>>>,
    /// Pending questions are locked after todos and never across I/O or await points.
    pub(super) questions: questions::QuestionRegistry,
    pub(super) session: Mutex<session::SessionState>,
    pub(super) agents: AgentRegistry,
    pub(super) dispatcher: ToolDispatcher,
    pub(super) subscribers: EventSubscribers,
    pub(super) active: Mutex<BTreeMap<u64, Arc<ActiveSubmission>>>,
    pub(super) auth_operations: Mutex<BTreeMap<ProviderId, Arc<AsyncMutex<()>>>>,
    pub(super) credential_operations: AsyncMutex<()>,
    pub(super) credential_states: Mutex<BTreeMap<ProviderId, ProviderCredentialState>>,
    pub(super) model_operations: AsyncMutex<()>,
    pub(super) model_refreshes: Mutex<BTreeMap<ProviderId, Arc<AsyncMutex<()>>>>,
    pub(super) submission_queue: Mutex<SubmissionQueue>,
    pub(super) submission_worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(super) next_submission: AtomicU64,
    pub(super) is_shutdown: AtomicBool,
}

#[derive(Debug)]
pub(super) struct ActiveSubmission {
    pub(super) cancelled: AtomicBool,
    pub(super) cancellation: watch::Sender<bool>,
    pub(super) request: Mutex<Option<(ProviderId, ProviderRequestId)>>,
}

impl ActiveSubmission {
    pub(super) fn new() -> Self {
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
        Self::build(
            paths,
            catalog,
            ProviderDeadlines::default(),
            CoreOptions::default(),
            None,
        )
    }

    fn build(
        paths: MisyPaths,
        catalog: ProviderCatalog,
        deadlines: ProviderDeadlines,
        options: CoreOptions,
        workspace_cwd: Option<std::path::PathBuf>,
    ) -> Result<Self, CoreError> {
        let instruction_root = match workspace_cwd {
            Some(workspace_cwd) => {
                instructions::InstructionRoot::load_at(&paths, false, &workspace_cwd)
            }
            None => instructions::InstructionRoot::load(&paths, false),
        }
        .map_err(CoreError::Runtime)?;
        let config_store = ConfigStore::new(paths.clone());
        let config = config_store.load()?;
        let credential_store = CredentialStore::new(paths.clone());
        let sessions_dir = paths.sessions_dir();
        let credential_states = credential_states(&catalog, &credential_store);
        let (runtime, owner) = RuntimeControl::new()?;
        let state = Arc::new(CoreState {
            self_reference: OnceLock::new(),
            credential_states: Mutex::new(credential_states),
            catalog: catalog.clone(),
            host: Arc::new(ProviderHost::with_handle_and_deadlines(
                catalog,
                runtime.handle.clone(),
                deadlines,
            )),
            config_store,
            credential_store,
            model_cache: Arc::new(ModelCatalogStore::new(paths.clone())),
            misy_paths: paths,
            instructions: Mutex::new(instructions::InstructionRuntime::new(instruction_root)),
            selected_model: Mutex::new(config.default_model),
            selected_thinking: Mutex::new(config.default_thinking),
            compaction_config: config.compaction,
            compaction: Mutex::new(None),
            keybindings: config.keybindings,
            client_capabilities: options.client_capabilities,
            history: Arc::new(Mutex::new(Vec::new())),
            active_history: Arc::new(Mutex::new(Vec::new())),
            todos: Arc::new(Mutex::new(Vec::new())),
            questions: questions::QuestionRegistry::new(),
            session: Mutex::new(session::SessionState::new(sessions_dir)?),
            agents: AgentRegistry::new(config.agents.max_concurrent_threads_per_session),
            dispatcher: ToolDispatcher::new(ToolRegistry::new()),
            subscribers: EventSubscribers::default(),
            active: Mutex::new(BTreeMap::new()),
            auth_operations: Mutex::new(BTreeMap::new()),
            credential_operations: AsyncMutex::new(()),
            model_operations: AsyncMutex::new(()),
            model_refreshes: Mutex::new(BTreeMap::new()),
            submission_queue: Mutex::new(SubmissionQueue::default()),
            submission_worker: Mutex::new(None),
            next_submission: AtomicU64::new(1),
            is_shutdown: AtomicBool::new(false),
        });
        state
            .self_reference
            .set(Arc::downgrade(&state))
            .map_err(|_| {
                CoreError::Runtime("could not initialize core self reference".to_owned())
            })?;
        owner.start(Arc::clone(&state))?;
        start_provider_event_router(
            &runtime.handle,
            Arc::clone(&state),
            state.host.subscribe_lossless(),
        );
        start_activity_event_router(&runtime.handle, Arc::clone(&state));
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

    /// Returns frontend-owned named keybinding overrides from the current configuration.
    pub fn keybindings(&self) -> BTreeMap<String, Vec<String>> {
        self.inner.state.keybindings.clone()
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

    /// Returns the atomically selected model and provider-owned reasoning level.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task poisoned the selected-thinking mutex.
    pub async fn selected_profile(&self) -> Option<ModelProfile> {
        let model = self.selected_model().await?;
        let thinking = self
            .inner
            .state
            .selected_thinking
            .lock()
            .expect("selected thinking mutex must not be poisoned")
            .clone();
        Some(ModelProfile::new(model, thinking))
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

    /// Requests cancellation of an active submission and forwards provider cancellation when set.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownSubmission`] when the submission is no longer active.
    pub async fn cancel(&self, submission: SubmissionId) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        self.inner.state.cancel_submission(submission).await
    }

    /// Returns bounded output for one command activity, optionally waiting for a state change.
    pub async fn activity_output(
        &self,
        id: ActivityId,
        wait: Option<std::time::Duration>,
    ) -> Option<ActivityOutput> {
        self.inner.state.dispatcher.activity_output(id, wait).await
    }

    /// Requests termination of one active command activity.
    pub fn stop_activity(&self, id: ActivityId) -> bool {
        if let Some(agent) = self
            .inner
            .state
            .agents
            .list()
            .into_iter()
            .find(|agent| agent.activity_id == id)
        {
            return self.inner.state.agents.stop_tree(agent.id).is_ok();
        }
        self.inner.state.dispatcher.stop_activity(id)
    }

    /// Returns the retained bounded transcript for one child agent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownAgent`] after an agent has been evicted or belongs to another
    /// root conversation.
    pub fn agent_transcript(&self, id: AgentId) -> Result<AgentTranscript, CoreError> {
        self.inner.state.agents.transcript(id)
    }

    /// Requests cancellation of one live child agent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UnknownAgent`] for an unknown child or
    /// [`CoreError::AgentAlreadyFinished`] for a terminal child.
    pub fn stop_agent(&self, id: AgentId) -> Result<(), CoreError> {
        self.inner.state.agents.stop(id)
    }

    /// Stops and explicitly discards all child-agent state for the current conversation.
    ///
    /// Clients must obtain user confirmation before calling this operation because unconsumed
    /// background results are removed.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or bounded child cleanup does not complete.
    pub async fn discard_agent_state(&self) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        self.inner.state.agents.close_admission();
        if !self
            .inner
            .state
            .agents
            .stop_all(std::time::Duration::from_secs(5))
            .await
        {
            self.inner.state.agents.reopen_admission();
            return Err(CoreError::Runtime(
                "child-agent cleanup did not complete before the deadline".to_owned(),
            ));
        }
        self.inner.state.agents.discard_current();
        self.inner.state.agents.reopen_admission();
        Ok(())
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
    pub(super) fn discover_roles(&self) -> BTreeMap<String, roles::AgentRoleSnapshot> {
        let project_root = self
            .instructions
            .lock()
            .expect("instruction runtime mutex must not be poisoned")
            .root()
            .project_root()
            .to_owned();
        roles::discover(
            &self.misy_paths,
            &project_root,
            &self.dispatcher.definition_names(),
        )
    }

    pub(super) fn weak_self(&self) -> Weak<CoreState> {
        self.self_reference.get().cloned().unwrap_or_default()
    }

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
        self.cancel_active_compaction().await;
        self.agents.close_admission();
        let _ = self
            .agents
            .stop_all(std::time::Duration::from_secs(5))
            .await;
        // Owner cancellation normally resolves these first. Draining afterward covers an
        // aborted owner task without waking it early enough to dispatch another local tool.
        for (request_id, sender) in self.questions.take_all() {
            self.emit(&CoreEvent::QuestionResolved { request_id });
            let _ = sender.send(questions::QuestionResolution::Cancelled);
        }
        self.dispatcher.shutdown().await;
        self.finish_submission_worker(std::time::Duration::from_secs(5))
            .await;
        // Provider shutdown is best effort; clients must still observe the Shutdown event.
        let _ = self.host.shutdown().await;
        self.subscribers.emit(&CoreEvent::Shutdown);
    }
}

fn start_activity_event_router(handle: &tokio::runtime::Handle, state: Arc<CoreState>) {
    let mut activities = state.dispatcher.subscribe_activities();
    handle.spawn(async move {
        while let Some(event) = activities.recv().await {
            match event {
                ActivityEvent::Changed(activity) => state
                    .subscribers
                    .emit(&CoreEvent::ActivityChanged { activity }),
                ActivityEvent::Finished(output) => state
                    .subscribers
                    .emit(&CoreEvent::ActivityFinished { output }),
                ActivityEvent::Flush(completion) => {
                    let _ = completion.send(());
                }
            }
        }
    });
}
