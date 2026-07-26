use super::{ProviderCatalog, ProviderError, ProviderEvent, ProviderPackage, ProviderRequestId};
use crate::ProviderId;
use command_group::{CommandGroup, GroupChild};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    process::{ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread,
};

/// A request already written to a provider. It can be cancelled while its response is pending.
#[derive(Debug)]
pub struct PendingProviderRequest {
    provider: ProviderId,
    id: ProviderRequestId,
    receiver: Receiver<Result<Value, PendingFailure>>,
}

impl PendingProviderRequest {
    pub fn id(&self) -> ProviderRequestId {
        self.id
    }

    pub fn wait(self) -> Result<Value, ProviderError> {
        match self.receiver.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(failure)) => Err(failure.into_error(self.provider, self.id)),
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
    lifecycle: Mutex<LifecycleState>,
    subscribers: Arc<Mutex<Vec<SyncSender<ProviderEvent>>>>,
    next_request_id: AtomicU64,
}

#[derive(Debug, Default)]
struct LifecycleState {
    processes: BTreeMap<String, Arc<ProviderProcess>>,
    is_shutdown: bool,
}

impl ProviderHost {
    pub fn new(catalog: ProviderCatalog) -> Self {
        Self {
            catalog,
            lifecycle: Mutex::new(LifecycleState::default()),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            next_request_id: AtomicU64::new(1),
        }
    }

    /// Registers a listener for all provider notifications.
    pub fn subscribe(&self) -> Receiver<ProviderEvent> {
        let (sender, receiver) = mpsc::sync_channel(64);
        self.subscribers
            .lock()
            .expect("provider subscribers mutex must not be poisoned")
            .push(sender);
        receiver
    }

    /// Sends a JSON-RPC request and returns a handle that waits for its correlated response.
    pub fn request_async(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<PendingProviderRequest, ProviderError> {
        let process = self.process_for(provider)?;
        let id = ProviderRequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let receiver = process.send_request(id, method, params)?;
        Ok(PendingProviderRequest {
            provider: provider.clone(),
            id,
            receiver,
        })
    }

    /// Sends a request and waits for its response.
    pub fn request(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        self.request_async(provider, method, params)?.wait()
    }

    /// Sends a JSON-RPC notification without allocating a response slot.
    pub fn notify(
        &self,
        provider: &ProviderId,
        method: &str,
        params: Value,
    ) -> Result<(), ProviderError> {
        self.process_for(provider)?
            .send_notification(method, params)
    }

    /// Cancels a pending request locally and forwards JSON-RPC cancellation to the provider.
    pub fn cancel_request(
        &self,
        provider: &ProviderId,
        id: ProviderRequestId,
    ) -> Result<(), ProviderError> {
        self.process_for(provider)?.cancel(id)
    }

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
    pub fn shutdown(&self) -> Result<(), ProviderError> {
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
            if let Err(error) = process.shutdown() {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn process_for(&self, provider: &ProviderId) -> Result<Arc<ProviderProcess>, ProviderError> {
        let mut lifecycle = self
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
        let package = self
            .catalog
            .get(provider.as_str())
            .ok_or_else(|| ProviderError::UnknownProvider(provider.as_str().to_owned()))?;
        let process = Arc::new(ProviderProcess::start(
            provider.clone(),
            package,
            Arc::clone(&self.subscribers),
        )?);
        lifecycle
            .processes
            .insert(provider.as_str().to_owned(), Arc::clone(&process));
        Ok(process)
    }
}

impl Drop for ProviderHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Debug)]
struct ProviderProcess {
    provider: ProviderId,
    io: Arc<Mutex<ProcessIo>>,
    state: TransportStateLock,
}

#[derive(Debug)]
struct ProcessIo {
    child: GroupChild,
    stdin: Option<ChildStdin>,
}

#[derive(Clone, Debug)]
enum PendingFailure {
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

type PendingSender = Sender<Result<Value, PendingFailure>>;
type TransportStateLock = Arc<Mutex<TransportState>>;

#[derive(Debug, Default)]
struct TransportState {
    pending: BTreeMap<u64, PendingSender>,
    failure: Option<PendingFailure>,
}

impl PendingFailure {
    fn into_error(self, provider: ProviderId, id: ProviderRequestId) -> ProviderError {
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

impl ProviderProcess {
    fn start(
        provider: ProviderId,
        package: &ProviderPackage,
        subscribers: Arc<Mutex<Vec<SyncSender<ProviderEvent>>>>,
    ) -> Result<Self, ProviderError> {
        let mut command = Command::new(&package.manifest().command);
        command
            .args(&package.manifest().args)
            .current_dir(package.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .group_spawn()
            .map_err(|error| ProviderError::Spawn {
                provider: provider.as_str().to_owned(),
                message: error.to_string(),
            })?;
        let stdout = child
            .inner()
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Spawn {
                provider: provider.as_str().to_owned(),
                message: "child stdout was not piped".to_owned(),
            })?;
        let stdin = child
            .inner()
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Spawn {
                provider: provider.as_str().to_owned(),
                message: "child stdin was not piped".to_owned(),
            })?;
        let process = Self {
            provider: provider.clone(),
            io: Arc::new(Mutex::new(ProcessIo {
                child,
                stdin: Some(stdin),
            })),
            state: Arc::new(Mutex::new(TransportState::default())),
        };
        let reader = process.reader_state();
        thread::spawn(move || reader_loop(provider, stdout, reader, subscribers));
        Ok(process)
    }

    fn reader_state(&self) -> ReaderState {
        ReaderState {
            state: self.state.clone(),
            io: self.io.clone(),
        }
    }

    fn is_failed(&self) -> bool {
        self.state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .failure
            .is_some()
    }

    fn send_request(
        &self,
        id: ProviderRequestId,
        method: &str,
        params: Value,
    ) -> Result<Receiver<Result<Value, PendingFailure>>, ProviderError> {
        let (sender, receiver) = mpsc::channel();
        {
            let mut state = self
                .state
                .lock()
                .expect("provider transport mutex must not be poisoned");
            if let Some(failure) = state.failure.clone() {
                return Err(failure.into_error(self.provider.clone(), id));
            }
            state.pending.insert(id.get(), sender);
        }
        let message = json!({
            "jsonrpc": "2.0",
            "id": id.get(),
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&message) {
            let failure = PendingFailure::Transport(error.to_string());
            self.fail(failure.clone());
            return Err(failure.into_error(self.provider.clone(), id));
        }
        Ok(receiver)
    }

    fn send_notification(&self, method: &str, params: Value) -> Result<(), ProviderError> {
        if let Some(failure) = self
            .state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .failure
            .clone()
        {
            return Err(failure.into_error(self.provider.clone(), ProviderRequestId(0)));
        }
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.write_message(&message).map_err(|error| {
            let failure = PendingFailure::Transport(error.to_string());
            self.fail(failure.clone());
            failure.into_error(self.provider.clone(), ProviderRequestId(0))
        })
    }

    fn cancel(&self, id: ProviderRequestId) -> Result<(), ProviderError> {
        let pending = self
            .state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .pending
            .remove(&id.get());
        if let Some(sender) = pending {
            let _ = sender.send(Err(PendingFailure::Cancelled));
        }
        self.send_notification("$/cancelRequest", json!({ "id": id.get() }))
    }

    fn write_message(&self, message: &Value) -> std::io::Result<()> {
        let mut encoded = serde_json::to_vec(message).map_err(std::io::Error::other)?;
        encoded.push(b'\n');
        let mut io = self
            .io
            .lock()
            .expect("provider process IO mutex must not be poisoned");
        let stdin = io.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "provider stdin is closed")
        })?;
        stdin.write_all(&encoded)?;
        stdin.flush()
    }

    fn fail(&self, failure: PendingFailure) {
        if fail_pending(&self.state, failure) {
            let _ = terminate_and_reap(&self.io);
        }
    }

    fn shutdown(&self) -> Result<(), ProviderError> {
        let _ = self.send_notification("misy.shutdown", Value::Null);
        self.fail(PendingFailure::Shutdown);
        terminate_and_reap(&self.io).map_err(|error| ProviderError::Transport {
            provider: self.provider.as_str().to_owned(),
            message: error.to_string(),
        })
    }
}

#[derive(Clone, Debug)]
struct ReaderState {
    state: TransportStateLock,
    io: Arc<Mutex<ProcessIo>>,
}

fn reader_loop(
    provider: ProviderId,
    stdout: impl std::io::Read,
    state: ReaderState,
    subscribers: Arc<Mutex<Vec<SyncSender<ProviderEvent>>>>,
) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                if fail_pending(&state.state, PendingFailure::Transport(error.to_string())) {
                    let _ = terminate_and_reap(&state.io);
                }
                return;
            }
        };
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                if fail_pending(&state.state, PendingFailure::Protocol(error.to_string())) {
                    let _ = terminate_and_reap(&state.io);
                }
                return;
            }
        };
        if let Err(failure) = route_message(&provider, message, &state.state, &subscribers) {
            if fail_pending(&state.state, failure) {
                let _ = terminate_and_reap(&state.io);
            }
            return;
        }
    }
    if fail_pending(
        &state.state,
        PendingFailure::Transport("provider closed stdout".to_owned()),
    ) {
        let _ = terminate_and_reap(&state.io);
    }
}

fn route_message(
    provider: &ProviderId,
    message: Value,
    state: &TransportStateLock,
    subscribers: &Arc<Mutex<Vec<SyncSender<ProviderEvent>>>>,
) -> Result<(), PendingFailure> {
    let object = message
        .as_object()
        .ok_or_else(|| PendingFailure::Protocol("message must be an object".to_owned()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(PendingFailure::Protocol(
            "jsonrpc must equal `2.0`".to_owned(),
        ));
    }
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if object.contains_key("id") {
            return Err(PendingFailure::Protocol(
                "provider requests are not supported".to_owned(),
            ));
        }
        broadcast(
            subscribers,
            ProviderEvent {
                provider: provider.clone(),
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            },
        );
        return Ok(());
    }
    let id = object.get("id").and_then(Value::as_u64).ok_or_else(|| {
        PendingFailure::Protocol("response id must be an unsigned integer".to_owned())
    })?;
    let response = match (object.get("result"), object.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => Err(parse_remote_error(error)?),
        _ => {
            return Err(PendingFailure::Protocol(
                "response must contain exactly one of result or error".to_owned(),
            ));
        }
    };
    if let Some(sender) = state
        .lock()
        .expect("provider transport mutex must not be poisoned")
        .pending
        .remove(&id)
    {
        let _ = sender.send(response);
    }
    Ok(())
}

fn parse_remote_error(value: &Value) -> Result<PendingFailure, PendingFailure> {
    let object = value
        .as_object()
        .ok_or_else(|| PendingFailure::Protocol("error must be an object".to_owned()))?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| PendingFailure::Protocol("error code must be an integer".to_owned()))?;
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| PendingFailure::Protocol("error message must be a string".to_owned()))?;
    Ok(PendingFailure::Remote {
        code,
        message: message.to_owned(),
        data: object.get("data").cloned(),
    })
}

/// Subscriber delivery is bounded and non-blocking. Full queues drop the new event;
/// disconnected subscribers are removed.
fn broadcast(subscribers: &Arc<Mutex<Vec<SyncSender<ProviderEvent>>>>, event: ProviderEvent) {
    subscribers
        .lock()
        .expect("provider subscribers mutex must not be poisoned")
        .retain(|sender| match sender.try_send(event.clone()) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
}

fn fail_pending(state: &TransportStateLock, failure: PendingFailure) -> bool {
    let pending = {
        let mut state = state
            .lock()
            .expect("provider transport mutex must not be poisoned");
        if state.failure.is_some() {
            return false;
        }
        state.failure = Some(failure.clone());
        std::mem::take(&mut state.pending)
    };
    for sender in pending.into_values() {
        let _ = sender.send(Err(failure.clone()));
    }
    true
}

fn terminate_and_reap(io: &Arc<Mutex<ProcessIo>>) -> std::io::Result<()> {
    let mut io = io
        .lock()
        .expect("provider process IO mutex must not be poisoned");
    io.stdin.take();
    if io.child.try_wait()?.is_none() {
        match io.child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(error),
        }
        let _ = io.child.wait()?;
    }
    Ok(())
}
