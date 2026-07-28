//! Ownership and shutdown coordination for the core's private Tokio runtime.

use super::{CoreError, CoreState};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::watch,
};

const DROP_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);

pub(super) struct RuntimeControl {
    pub(super) handle: Handle,
    shutdown: watch::Sender<bool>,
    completion: watch::Receiver<bool>,
    completed: Arc<(Mutex<bool>, Condvar)>,
}

pub(super) struct RuntimeOwner {
    runtime: Runtime,
    shutdown: watch::Receiver<bool>,
    completion: watch::Sender<bool>,
    completed: Arc<(Mutex<bool>, Condvar)>,
}

impl RuntimeControl {
    pub(super) fn new() -> Result<(Self, RuntimeOwner), CoreError> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| CoreError::Runtime(error.to_string()))?;
        let handle = runtime.handle().clone();
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (completion_sender, completion) = watch::channel(false);
        let completed = Arc::new((Mutex::new(false), Condvar::new()));
        let completed_for_owner = Arc::clone(&completed);
        Ok((
            Self {
                handle,
                shutdown,
                completion,
                completed,
            },
            RuntimeOwner {
                runtime,
                shutdown: shutdown_receiver,
                completion: completion_sender,
                completed: completed_for_owner,
            },
        ))
    }

    pub(super) fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub(super) async fn wait_for_shutdown(&self) {
        let mut completion = self.completion.clone();
        while !*completion.borrow() {
            if completion.changed().await.is_err() {
                return;
            }
        }
    }

    pub(super) fn wait_for_drop(&self) {
        let (complete, wake) = &*self.completed;
        let completed = complete
            .lock()
            .expect("runtime completion mutex must not be poisoned");
        if *completed {
            return;
        }
        // Drop cannot await: callers may release the last core handle from an arbitrary runtime.
        // An elapsed grace only means teardown is slow; the runtime is dropped either way.
        let _ = wake
            .wait_timeout(completed, DROP_SHUTDOWN_GRACE)
            .expect("runtime completion mutex must not be poisoned");
    }
}

impl RuntimeOwner {
    pub(super) fn start(self, state: Arc<CoreState>) -> Result<(), CoreError> {
        thread::Builder::new()
            .name("misy-runtime-owner".to_owned())
            .spawn(move || {
                run_runtime_owner(
                    self.runtime,
                    state,
                    self.shutdown,
                    &self.completion,
                    &self.completed,
                );
            })
            .map(|_| ())
            .map_err(|error| CoreError::Runtime(error.to_string()))
    }
}

fn run_runtime_owner(
    runtime: Runtime,
    state: Arc<CoreState>,
    mut shutdown: watch::Receiver<bool>,
    completion: &watch::Sender<bool>,
    completed: &Arc<(Mutex<bool>, Condvar)>,
) {
    runtime.block_on(async move {
        if !*shutdown.borrow() {
            // A closed watch means the control side is gone, which itself implies shutdown.
            let _ = shutdown.changed().await;
        }
        state.shutdown_services().await;
    });
    runtime.shutdown_timeout(DROP_SHUTDOWN_GRACE);
    // Completion is visible only after runtime-owned tasks have received their bounded teardown.
    // The send fails only when no control handle remains to observe it.
    let _ = completion.send(true);
    let (complete, wake) = &**completed;
    *complete
        .lock()
        .expect("runtime completion mutex must not be poisoned") = true;
    wake.notify_all();
}
