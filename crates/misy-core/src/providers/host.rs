//! Asynchronous supervision of provider subprocesses and their JSON-RPC transport.

mod process;
mod router;

use super::{ProviderCatalog, ProviderError, ProviderEvent, ProviderRequestId};
use crate::{ProviderId, fanout::Fanout};
use process::ProviderProcess;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

pub(crate) type ProviderStreamReceiver =
    mpsc::UnboundedReceiver<Result<ProviderEvent, PendingFailure>>;

/// Deadlines the host applies while waiting on provider subprocesses.
///
/// A provider is an external process that can hang without dying, so every wait on it is
/// bounded: [`ProviderHost::request`] applies [`ProviderDeadlines::for_method`], and the core
/// applies `stream_idle` between events of a `chat.start` stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDeadlines {
    /// Deadline for requests that do not wait on interactive user action.
    pub request: Duration,
    /// Deadline for `auth.*` requests, which block while the user authenticates in a browser.
    pub auth: Duration,
    /// Maximum silence between stream events before the core abandons the turn.
    pub stream_idle: Duration,
}

impl ProviderDeadlines {
    /// Returns the deadline [`ProviderHost::request`] applies for `method`.
    ///
    /// Interactive `auth.*` flows wait on the user rather than only on the provider, so they get
    /// the longer [`ProviderDeadlines::auth`] bound instead of the regular request bound.
    pub fn for_method(&self, method: &str) -> Duration {
        if method.starts_with("auth.") {
            self.auth
        } else {
            self.request
        }
    }
}

impl Default for ProviderDeadlines {
    fn default() -> Self {
        Self {
            // Two minutes covers slow but healthy model discovery and status round trips.
            request: Duration::from_secs(120),
            // Browser and device auth flows block until the user finishes, which takes minutes.
            auth: Duration::from_secs(600),
            // A model may think for a long while between deltas, so the idle bound stays generous:
            // it catches a hung subprocess, it must not pace legitimate long streams.
            stream_idle: Duration::from_secs(120),
        }
    }
}

/// A request already written to a provider. It can be cancelled while its response is pending.
#[derive(Debug)]
pub struct PendingProviderRequest {
    provider: ProviderId,
    id: ProviderRequestId,
    receiver: oneshot::Receiver<Result<Value, PendingFailure>>,
}

/// A streaming chat with independently correlated response and notification routes.
#[derive(Debug)]
pub struct PendingProviderChat {
    request: PendingProviderRequest,
    events: ProviderStreamReceiver,
}

impl PendingProviderChat {
    /// Returns the host-assigned request identifier shared by the response and stream route.
    pub fn id(&self) -> ProviderRequestId {
        self.request.id()
    }

    /// Waits for the next event from this chat's correlated stream.
    ///
    /// # Errors
    ///
    /// Returns request-scoped or process-wide provider failures. `Ok(None)` means a terminal
    /// notification closed the stream route after its final event.
    #[cfg(feature = "test-support")]
    pub async fn next_event(&mut self) -> Result<Option<ProviderEvent>, ProviderError> {
        match self.events.recv().await {
            Some(Ok(event)) => Ok(Some(event)),
            Some(Err(failure)) => Err(failure.into_error(&self.request.provider, self.request.id)),
            None => Ok(None),
        }
    }

    /// Waits for the JSON-RPC response paired with this chat stream.
    ///
    /// # Errors
    ///
    /// Returns transport, protocol, remote, cancellation, or shutdown failures from the paired
    /// request response.
    #[cfg(feature = "test-support")]
    pub async fn wait_response(self) -> Result<Value, ProviderError> {
        self.request.wait().await
    }

    /// Separates the response waiter from its already-registered stream receiver.
    pub(crate) fn into_parts(self) -> (PendingProviderRequest, ProviderStreamReceiver) {
        (self.request, self.events)
    }
}

impl PendingProviderRequest {
    /// Returns the host-assigned JSON-RPC request identifier.
    pub fn id(&self) -> ProviderRequestId {
        self.id
    }

    /// Waits for the request's provider response.
    ///
    /// The wait itself is unbounded; [`ProviderHost::request`] bounds its wait with a deadline,
    /// and the core bounds `chat.start` stream collection with a stream idle deadline.
    ///
    /// # Errors
    ///
    /// Returns a provider transport, protocol, remote, cancellation, or shutdown error when the
    /// response cannot be delivered successfully.
    pub async fn wait(self) -> Result<Value, ProviderError> {
        match self.receiver.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(failure)) => Err(failure.into_error(&self.provider, self.id)),
            Err(_) => Err(ProviderError::Transport {
                provider: self.provider.as_str().to_owned(),
                message: "response channel closed unexpectedly".to_owned(),
            }),
        }
    }
}

/// Supervises lazy, long-lived provider subprocesses and their JSON-RPC transport.
#[derive(Debug)]
pub struct ProviderHost {
    catalog: ProviderCatalog,
    deadlines: ProviderDeadlines,
    // This lock only protects process-map transitions; no async I/O occurs while it is held.
    lifecycle: Mutex<LifecycleState>,
    // Only the test-support bounded `subscribe` registers lossy subscribers; the core itself
    // always subscribes lossless.
    subscribers: Arc<Fanout<ProviderEvent>>,
    task_handle: Handle,
    next_request_id: AtomicU64,
}

#[derive(Debug, Default)]
struct LifecycleState {
    processes: BTreeMap<ProviderId, Arc<ProviderProcess>>,
    is_shutdown: bool,
}

impl ProviderHost {
    /// Creates a host whose provider tasks run on `task_handle`.
    ///
    /// The handle is mandatory so provider I/O stays on the runtime the owner chose: the core
    /// injects its owned runtime handle, keeping provider work independent of any client runtime.
    #[cfg(feature = "test-support")]
    pub fn with_handle(catalog: ProviderCatalog, task_handle: Handle) -> Self {
        Self::build(catalog, task_handle, ProviderDeadlines::default())
    }

    /// Creates a host with an explicit runtime handle and explicit wait deadlines.
    ///
    /// Integration tests use this to keep hung-provider scenarios fast; production clients
    /// should prefer [`ProviderDeadlines::default`] through [`ProviderHost::with_handle`].
    pub fn with_handle_and_deadlines(
        catalog: ProviderCatalog,
        task_handle: Handle,
        deadlines: ProviderDeadlines,
    ) -> Self {
        Self::build(catalog, task_handle, deadlines)
    }

    fn build(catalog: ProviderCatalog, task_handle: Handle, deadlines: ProviderDeadlines) -> Self {
        Self {
            catalog,
            deadlines,
            lifecycle: Mutex::new(LifecycleState::default()),
            subscribers: Arc::new(Fanout::default()),
            task_handle,
            next_request_id: AtomicU64::new(1),
        }
    }

    /// Registers a bounded listener for provider notifications.
    ///
    /// New events are dropped when the listener has not consumed its 64-event buffer.
    ///
    /// # Panics
    ///
    /// Panics if the subscriber mutex is poisoned by an earlier host-task panic.
    #[cfg(feature = "test-support")]
    pub fn subscribe(&self) -> mpsc::Receiver<ProviderEvent> {
        self.subscribers.subscribe(64, std::iter::empty())
    }

    /// Subscribes a request-correlated consumer without dropping stream events.
    ///
    /// # Panics
    ///
    /// Panics if the subscriber mutex is poisoned by an earlier host-task panic.
    pub fn subscribe_lossless(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        self.subscribers.subscribe_lossless(std::iter::empty())
    }

    /// Sends a JSON-RPC request and returns a handle that waits for its correlated response.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is unknown, shut down, cannot start, or cannot accept
    /// the request.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    #[cfg(feature = "test-support")]
    pub async fn request_async(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<PendingProviderRequest, ProviderError> {
        let process = self.process_for(provider).await?;
        let id = ProviderRequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let receiver = process.send_request(id, method, params).await?;
        Ok(PendingProviderRequest {
            provider: provider.clone(),
            id,
            receiver,
        })
    }

    /// Starts a streaming chat after atomically registering its response and stream routes.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is unknown, shut down, cannot start, or rejects the
    /// request write. The stream receiver receives request-scoped protocol failures, while an
    /// uncorrelatable framing failure terminates the provider and fails every pending chat.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    pub async fn start_chat(
        &self,
        provider: &ProviderId,
        params: Value,
    ) -> Result<PendingProviderChat, ProviderError> {
        let process = self.process_for(provider).await?;
        let id = ProviderRequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (receiver, events) = process.send_chat(id, params).await?;
        Ok(PendingProviderChat {
            request: PendingProviderRequest {
                provider: provider.clone(),
                id,
                receiver,
            },
            events,
        })
    }

    /// Sends a request and waits for its response with a host-enforced deadline.
    ///
    /// The wait is bounded by [`ProviderDeadlines::for_method`]. A provider that outlives the
    /// deadline is reaped exactly as by [`ProviderHost::request_with_timeout`], so the next
    /// request starts a fresh process instead of hanging behind the stuck one.
    ///
    /// # Errors
    ///
    /// Returns any error from request submission or response delivery, or
    /// [`ProviderError::Timeout`] when the provider does not answer within the deadline.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    pub async fn request(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let deadline = self.deadlines.for_method(method);
        self.request_with_timeout(provider, method, params, deadline)
            .await
    }

    /// Sends a request with a host-enforced deadline and reaps a provider that stops responding.
    ///
    /// # Errors
    ///
    /// Returns any request error, or [`ProviderError::Timeout`] after `timeout` elapses.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    pub async fn request_with_timeout(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ProviderError> {
        let process = self.process_for(provider).await?;
        let id = ProviderRequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let receiver = process.send_request(id, method, params).await?;
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(failure))) => Err(failure.into_error(provider, id)),
            Ok(Err(_)) => Err(ProviderError::Transport {
                provider: provider.as_str().to_owned(),
                message: "response channel closed unexpectedly".to_owned(),
            }),
            Err(_) => {
                process
                    .fail(PendingFailure::Transport(format!(
                        "provider request `{method}` timed out"
                    )))
                    .await;
                Err(ProviderError::Timeout {
                    provider: provider.as_str().to_owned(),
                    method: method.to_owned(),
                })
            }
        }
    }

    /// Cancels a pending request locally and forwards JSON-RPC cancellation to the provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is unavailable or cancellation cannot be written.
    pub async fn cancel_request(
        &self,
        provider: &ProviderId,
        id: ProviderRequestId,
    ) -> Result<(), ProviderError> {
        self.process_for(provider).await?.cancel(id).await
    }

    /// Returns the deadlines this host applies while waiting on providers.
    pub(crate) fn deadlines(&self) -> ProviderDeadlines {
        self.deadlines
    }

    /// Fails the provider's live process, if any, so the next request starts a fresh one.
    ///
    /// Unlike [`ProviderHost::request_with_timeout`], which fails the process that outlives its
    /// own request, this covers caller-enforced deadlines — the core's stream idle bound —
    /// where only the host can reach the process.
    pub(crate) async fn fail_provider(&self, provider: &ProviderId, reason: String) {
        let process = self
            .lifecycle
            .lock()
            .expect("provider lifecycle mutex must not be poisoned")
            .processes
            .get(provider)
            .cloned();
        if let Some(process) = process {
            process.fail(PendingFailure::Transport(reason)).await;
        }
    }

    /// Returns the number of healthy child processes currently running.
    ///
    /// # Panics
    ///
    /// Panics if the lifecycle mutex is poisoned by an earlier host-task panic.
    pub fn running_provider_count(&self) -> usize {
        self.lifecycle
            .lock()
            .expect("provider lifecycle mutex must not be poisoned")
            .processes
            .values()
            .filter(|process| !process.is_failed())
            .count()
    }

    /// Stops every child, failing outstanding requests and closing their stdin streams.
    ///
    /// # Errors
    ///
    /// Returns the first child-process termination error after attempting every shutdown.
    ///
    /// # Panics
    ///
    /// Panics if the lifecycle mutex is poisoned by an earlier host-task panic.
    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        let processes = {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .expect("provider lifecycle mutex must not be poisoned");
            if lifecycle.is_shutdown {
                return Ok(());
            }
            lifecycle.is_shutdown = true;
            std::mem::take(&mut lifecycle.processes)
        };
        let mut first_error = None;
        for process in processes.into_values() {
            if let Err(error) = process.shutdown().await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn process_for(
        &self,
        provider: &ProviderId,
    ) -> Result<Arc<ProviderProcess>, ProviderError> {
        let package = {
            let lifecycle = self
                .lifecycle
                .lock()
                .expect("provider lifecycle mutex must not be poisoned");
            if lifecycle.is_shutdown {
                return Err(ProviderError::Shutdown);
            }
            if let Some(process) = lifecycle.processes.get(provider)
                && !process.is_failed()
            {
                return Ok(Arc::clone(process));
            }
            self.catalog
                .get(provider)
                .cloned()
                .ok_or_else(|| ProviderError::UnknownProvider(provider.as_str().to_owned()))?
        };
        let task_handle = self.task_handle.clone();
        let process = Arc::new(ProviderProcess::start(
            provider.clone(),
            &package,
            Arc::clone(&self.subscribers),
            &task_handle,
        )?);
        let replacement = {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .expect("provider lifecycle mutex must not be poisoned");
            if lifecycle.is_shutdown {
                None
            } else if let Some(existing) = lifecycle.processes.get(provider)
                && !existing.is_failed()
            {
                Some(Arc::clone(existing))
            } else {
                lifecycle
                    .processes
                    .insert(provider.clone(), Arc::clone(&process));
                return Ok(process);
            }
        };
        // This process lost the startup race; kill_on_drop reaps it even if shutdown fails.
        let _ = process.shutdown().await;
        replacement.ok_or(ProviderError::Shutdown)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PendingFailure {
    Transport(String),
    Protocol(String),
    Remote {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    Cancelled,
    Shutdown,
}

impl PendingFailure {
    pub(crate) fn into_error(self, provider: &ProviderId, id: ProviderRequestId) -> ProviderError {
        match self {
            Self::Transport(message) => ProviderError::Transport {
                provider: provider.as_str().to_owned(),
                message,
            },
            Self::Protocol(message) => ProviderError::Protocol {
                provider: provider.as_str().to_owned(),
                message,
            },
            Self::Remote {
                code,
                message,
                data,
            } => ProviderError::Remote {
                provider: provider.as_str().to_owned(),
                code,
                message,
                data,
            },
            Self::Cancelled => ProviderError::Cancelled(id),
            Self::Shutdown => ProviderError::Shutdown,
        }
    }
}

impl Drop for ProviderHost {
    fn drop(&mut self) {
        let processes = match self.lifecycle.get_mut() {
            Ok(lifecycle) => {
                if lifecycle.is_shutdown {
                    return;
                }
                lifecycle.is_shutdown = true;
                std::mem::take(&mut lifecycle.processes)
            }
            Err(_) => return,
        };
        // Each process signals its group synchronously in Drop, which remains safe after the
        // caller runtime stops. Explicit shutdown performs the asynchronous reap and reports it.
        drop(processes);
    }
}
