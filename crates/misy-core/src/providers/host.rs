//! Asynchronous supervision of provider subprocesses and their JSON-RPC transport.

mod process;
mod router;

use super::{ProviderCatalog, ProviderError, ProviderEvent, ProviderRequestId};
use crate::ProviderId;
use process::ProviderProcess;
use router::ProviderSubscriber;
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

/// A request already written to a provider. It can be cancelled while its response is pending.
#[derive(Debug)]
pub struct PendingProviderRequest {
    provider: ProviderId,
    id: ProviderRequestId,
    receiver: oneshot::Receiver<Result<Value, PendingFailure>>,
}

impl PendingProviderRequest {
    /// Returns the host-assigned JSON-RPC request identifier.
    pub fn id(&self) -> ProviderRequestId {
        self.id
    }

    /// Waits for the request's provider response.
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
    // This lock only protects process-map transitions; no async I/O occurs while it is held.
    lifecycle: Mutex<LifecycleState>,
    subscribers: Arc<Mutex<Vec<ProviderSubscriber>>>,
    task_handle: Option<Handle>,
    next_request_id: AtomicU64,
}

#[derive(Debug, Default)]
struct LifecycleState {
    processes: BTreeMap<String, Arc<ProviderProcess>>,
    is_shutdown: bool,
}

impl ProviderHost {
    /// Creates a host that lazily launches packages from `catalog`.
    ///
    /// Provider work starts on the calling Tokio runtime. Use [`ProviderHost::with_handle`] when
    /// the host must remain independent of the caller runtime.
    pub fn new(catalog: ProviderCatalog) -> Self {
        Self {
            catalog,
            lifecycle: Mutex::new(LifecycleState::default()),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            task_handle: None,
            next_request_id: AtomicU64::new(1),
        }
    }

    /// Creates a host whose provider tasks run on `task_handle`.
    ///
    /// The core injects its owned runtime handle so provider I/O remains independent of a
    /// client's Tokio runtime. Standalone users can keep [`ProviderHost::new`] and call it from
    /// within their runtime instead.
    pub fn with_handle(catalog: ProviderCatalog, task_handle: Handle) -> Self {
        Self {
            catalog,
            lifecycle: Mutex::new(LifecycleState::default()),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            task_handle: Some(task_handle),
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
    pub fn subscribe(&self) -> mpsc::Receiver<ProviderEvent> {
        let (sender, receiver) = mpsc::channel(64);
        self.subscribers
            .lock()
            .expect("provider subscribers mutex must not be poisoned")
            .push(ProviderSubscriber::Lossy(sender));
        receiver
    }

    /// Subscribes a request-correlated consumer without dropping stream events.
    ///
    /// # Panics
    ///
    /// Panics if the subscriber mutex is poisoned by an earlier host-task panic.
    pub fn subscribe_lossless(&self) -> mpsc::UnboundedReceiver<ProviderEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .expect("provider subscribers mutex must not be poisoned")
            .push(ProviderSubscriber::Lossless(sender));
        receiver
    }

    /// Sends a JSON-RPC request and returns a handle that waits for its correlated response.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is unknown, shut down, cannot start, or cannot accept
    /// the request.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
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

    /// Sends a request and waits for its response.
    ///
    /// # Errors
    ///
    /// Returns any error from request submission or response delivery.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    pub async fn request(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        self.request_async(provider, method, params)
            .await?
            .wait()
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

    /// Sends a JSON-RPC notification without allocating a response slot.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is unavailable or the notification cannot be written.
    #[allow(clippy::needless_pass_by_value)] // The public contract transfers opaque JSON.
    pub async fn notify(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<(), ProviderError> {
        self.process_for(provider)
            .await?
            .send_notification(method, params)
            .await
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
            if let Some(process) = lifecycle.processes.get(provider.as_str())
                && !process.is_failed()
            {
                return Ok(Arc::clone(process));
            }
            self.catalog
                .get(provider.as_str())
                .cloned()
                .ok_or_else(|| ProviderError::UnknownProvider(provider.as_str().to_owned()))?
        };
        let task_handle = self.task_handle(provider)?;
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
            } else if let Some(existing) = lifecycle.processes.get(provider.as_str())
                && !existing.is_failed()
            {
                Some(Arc::clone(existing))
            } else {
                lifecycle
                    .processes
                    .insert(provider.as_str().to_owned(), Arc::clone(&process));
                return Ok(process);
            }
        };
        let _ = process.shutdown().await;
        replacement.ok_or(ProviderError::Shutdown)
    }

    fn task_handle(&self, provider: &ProviderId) -> Result<Handle, ProviderError> {
        if let Some(task_handle) = &self.task_handle {
            return Ok(task_handle.clone());
        }
        Handle::try_current().map_err(|_| ProviderError::Spawn {
            provider: provider.as_str().to_owned(),
            message: "provider host requires a Tokio runtime".to_owned(),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) enum PendingFailure {
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
    fn into_error(self, provider: &ProviderId, id: ProviderRequestId) -> ProviderError {
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
