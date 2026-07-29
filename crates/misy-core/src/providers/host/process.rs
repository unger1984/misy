//! Provider subprocess lifecycle and asynchronous stdin/stdout transport.

use super::{
    PendingFailure, ProviderEvent,
    router::{TransportStateLock, fail_pending, route_message},
};
use crate::{
    ProviderError, ProviderId, ProviderPackage, ProviderRequestId, fanout::Fanout,
    providers::protocol::MAX_PROTOCOL_FRAME_BYTES,
};
use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::{Value, json};
use std::{
    io,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    runtime::Handle,
    sync::{Mutex as AsyncMutex, oneshot, watch},
    time::timeout,
};

const REAPER_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns one provider child and the state required to route its JSON-RPC responses.
#[derive(Debug)]
pub(super) struct ProviderProcess {
    provider: ProviderId,
    // The synchronous kill handle survives caller-runtime teardown for Drop fallback cleanup.
    child: Arc<Mutex<Option<AsyncGroupChild>>>,
    // Serializes stdin writes with process termination so no command races a closed stream.
    io: Arc<AsyncMutex<ProcessIo>>,
    state: TransportStateLock,
}

#[derive(Debug)]
struct ProcessIo {
    stdin: Option<ChildStdin>,
    reaper: Option<watch::Receiver<Option<Result<(), String>>>>,
}

impl ProviderProcess {
    pub(super) fn start(
        provider: ProviderId,
        package: &ProviderPackage,
        subscribers: Arc<Fanout<ProviderEvent>>,
        task_handle: &Handle,
    ) -> Result<Self, ProviderError> {
        let mut command = Command::new(&package.manifest().command);
        command
            .args(&package.manifest().args)
            .current_dir(package.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
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
            .ok_or_else(|| spawn_error(&provider, "child stdout was not piped"))?;
        let stdin = child
            .inner()
            .stdin
            .take()
            .ok_or_else(|| spawn_error(&provider, "child stdin was not piped"))?;
        let process = Self {
            provider: provider.clone(),
            child: Arc::new(Mutex::new(Some(child))),
            io: Arc::new(AsyncMutex::new(ProcessIo {
                stdin: Some(stdin),
                reaper: None,
            })),
            state: Arc::new(Mutex::new(Default::default())),
        };
        task_handle.spawn(reader_loop(
            provider,
            stdout,
            Arc::clone(&process.io),
            Arc::clone(&process.child),
            Arc::clone(&process.state),
            subscribers,
        ));
        Ok(process)
    }

    pub(super) fn is_failed(&self) -> bool {
        self.state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .failure
            .is_some()
    }

    pub(super) async fn send_request(
        &self,
        id: ProviderRequestId,
        method: &str,
        params: Value,
    ) -> Result<oneshot::Receiver<Result<Value, PendingFailure>>, ProviderError> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .expect("provider transport mutex must not be poisoned");
            if let Some(failure) = state.failure.clone() {
                return Err(failure.into_error(&self.provider, id));
            }
            state.pending.insert(id.get(), sender);
        }
        let message = json!({
            "jsonrpc": "2.0",
            "id": id.get(),
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&message).await {
            let failure = PendingFailure::Transport(error.to_string());
            self.fail(failure.clone()).await;
            return Err(failure.into_error(&self.provider, id));
        }
        Ok(receiver)
    }

    pub(super) async fn send_notification(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(), ProviderError> {
        if let Some(failure) = self
            .state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .failure
            .clone()
        {
            return Err(failure.into_error(&self.provider, ProviderRequestId(0)));
        }
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if let Err(error) = self.write_message(&message).await {
            let failure = PendingFailure::Transport(error.to_string());
            self.fail(failure.clone()).await;
            return Err(failure.into_error(&self.provider, ProviderRequestId(0)));
        }
        Ok(())
    }

    pub(super) async fn cancel(&self, id: ProviderRequestId) -> Result<(), ProviderError> {
        let pending = self
            .state
            .lock()
            .expect("provider transport mutex must not be poisoned")
            .pending
            .remove(&id.get());
        if let Some(sender) = pending {
            // The waiter may have dropped its receiver after a racing response; cancel stands.
            let _ = sender.send(Err(PendingFailure::Cancelled));
        }
        self.send_notification("chat.cancel", json!({ "request_id": id.get() }))
            .await
    }

    pub(super) async fn fail(&self, failure: PendingFailure) {
        if fail_pending(&self.state, &failure) {
            // Pending requests are already failed; kill_on_drop and Drop reap the child anyway.
            let _ = self.begin_termination().await;
        }
    }

    pub(super) async fn shutdown(&self) -> Result<(), ProviderError> {
        // The shutdown notice is a courtesy; termination below does not depend on its delivery.
        let _ = self.send_notification("misy.shutdown", Value::Null).await;
        self.fail(PendingFailure::Shutdown).await;
        self.begin_termination()
            .await
            .map_err(|error| ProviderError::Transport {
                provider: self.provider.as_str().to_owned(),
                message: error.to_string(),
            })?;
        self.wait_for_reaper()
            .await
            .map_err(|error| ProviderError::Transport {
                provider: self.provider.as_str().to_owned(),
                message: error.to_string(),
            })
    }

    async fn write_message(&self, message: &Value) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(message).map_err(io::Error::other)?;
        encoded.push(b'\n');
        if encoded.len() > MAX_PROTOCOL_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "provider protocol message exceeds 32 MiB limit",
            ));
        }
        // A provider has one ordered stdin stream, so the lock deliberately spans the async write.
        let mut io = self.io.lock().await;
        let stdin = io
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "provider stdin is closed"))?;
        stdin.write_all(&encoded).await?;
        stdin.flush().await
    }

    async fn begin_termination(&self) -> io::Result<()> {
        begin_termination(&self.io, &self.child).await
    }

    async fn wait_for_reaper(&self) -> io::Result<()> {
        let Some(mut reaper) = self.io.lock().await.reaper.clone() else {
            return Ok(());
        };
        if let Some(result) = reaper.borrow().clone() {
            return result.map_err(io::Error::other);
        }
        match timeout(REAPER_WAIT_TIMEOUT, reaper.changed()).await {
            Ok(Ok(())) => reaper
                .borrow()
                .clone()
                .ok_or_else(|| io::Error::other("provider reaper completed without a result"))?
                .map_err(io::Error::other),
            Ok(Err(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "provider reaper stopped before reporting completion",
            )),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "provider process did not exit before the reaper deadline",
            )),
        }
    }
}

async fn reader_loop(
    provider: ProviderId,
    stdout: ChildStdout,
    io: Arc<AsyncMutex<ProcessIo>>,
    child: Arc<Mutex<Option<AsyncGroupChild>>>,
    state: TransportStateLock,
    subscribers: Arc<Fanout<ProviderEvent>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let line = match read_protocol_line(&mut reader).await {
            Ok(line) => line,
            Err(failure) => {
                fail_and_reap(&state, &io, &child, failure).await;
                return;
            }
        };
        let message = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                fail_and_reap(
                    &state,
                    &io,
                    &child,
                    PendingFailure::Protocol(error.to_string()),
                )
                .await;
                return;
            }
        };
        if let Err(failure) = route_message(&provider, &message, &state, &subscribers) {
            fail_and_reap(&state, &io, &child, failure).await;
            return;
        }
    }
}

/// Reads one protocol line with a hard size cap. Reading through `take` bounds how much a
/// single line accumulates; once the process is reaped anyway, the unread remainder of an
/// oversized line does not matter, so it is deliberately not drained. End of stream is an
/// error: the transport cannot outlive the provider's stdout.
async fn read_protocol_line(reader: &mut BufReader<ChildStdout>) -> Result<String, PendingFailure> {
    let mut line = String::new();
    let read = reader
        .take(MAX_PROTOCOL_FRAME_BYTES as u64 + 1)
        .read_line(&mut line)
        .await;
    match read {
        Ok(0) => Err(PendingFailure::Transport(
            "provider closed stdout".to_owned(),
        )),
        Ok(_) if line.len() > MAX_PROTOCOL_FRAME_BYTES => Err(PendingFailure::Protocol(format!(
            "protocol line exceeds the {MAX_PROTOCOL_FRAME_BYTES}-byte limit"
        ))),
        Ok(_) => Ok(line),
        Err(error) => Err(PendingFailure::Transport(error.to_string())),
    }
}

async fn fail_and_reap(
    state: &TransportStateLock,
    io: &Arc<AsyncMutex<ProcessIo>>,
    child: &Arc<Mutex<Option<AsyncGroupChild>>>,
    failure: PendingFailure,
) {
    if fail_pending(state, &failure) {
        // Pending requests are already failed; kill_on_drop and Drop reap the child anyway.
        let _ = begin_termination(io, child).await;
    }
}

async fn begin_termination(
    io: &Arc<AsyncMutex<ProcessIo>>,
    child: &Arc<Mutex<Option<AsyncGroupChild>>>,
) -> io::Result<()> {
    let mut io = io.lock().await;
    io.stdin.take();
    if io.reaper.is_some() {
        return Ok(());
    }
    let Some(mut child) = child
        .lock()
        .expect("provider child mutex must not be poisoned")
        .take()
    else {
        return Ok(());
    };
    match child.start_kill() {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
        Err(error) => return Err(error),
    }
    let (completion_sender, completion_receiver) = watch::channel(None);
    spawn_cleanup_owner(child, Some(completion_sender));
    io.reaper = Some(completion_receiver);
    Ok(())
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        if let Some(mut child) = child.take() {
            // Drop cannot await reaping, so an independent owner keeps the sole wait alive after
            // the caller runtime has stopped. If thread creation fails, start_kill is still the
            // best-effort group cleanup and kill_on_drop remains a final leader-process fallback.
            let _ = child.start_kill();
            spawn_cleanup_owner(child, None);
        }
    }
}

fn spawn_cleanup_owner(
    mut child: AsyncGroupChild,
    completion_sender: Option<watch::Sender<Option<Result<(), String>>>>,
) {
    let completion_for_owner = completion_sender.clone();
    let thread = std::thread::Builder::new()
        .name("misy-provider-reaper".to_owned())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(async { child.wait().await })
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                });
            if let Some(completion_sender) = completion_for_owner {
                // Fails only when no reaper waiter remains; the child is already reaped.
                let _ = completion_sender.send(Some(result));
            }
        });
    if let (Err(error), Some(completion_sender)) = (thread, completion_sender) {
        // Fails only when no reaper waiter remains to observe the spawn failure.
        let _ = completion_sender.send(Some(Err(error.to_string())));
    }
}

fn spawn_error(provider: &ProviderId, message: &str) -> ProviderError {
    ProviderError::Spawn {
        provider: provider.as_str().to_owned(),
        message: message.to_owned(),
    }
}
