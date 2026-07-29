//! Process-group supervision and bounded stdout/stderr capture.

use super::{ActivityEvent, ActivityManagerInner, ActivityRecord, StopReason, retain_recent};
use crate::{ActivityOutputStream, ActivityStatus};
use command_group::AsyncGroupChild;
use std::{
    io,
    process::ExitStatus,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
    task::JoinHandle,
    time::{Instant, sleep_until, timeout},
};

const PROCESS_REAP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const CAPTURE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct SpawnedStreams {
    pub(super) stdout: Option<JoinHandle<io::Result<()>>>,
    pub(super) stderr: Option<JoinHandle<io::Result<()>>>,
}

pub(super) fn spawn_supervisor(
    manager: Weak<ActivityManagerInner>,
    record: Arc<ActivityRecord>,
    mut child: AsyncGroupChild,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    mut stop: watch::Receiver<Option<StopReason>>,
    command_timeout: Option<Duration>,
) -> JoinHandle<()> {
    let streams = SpawnedStreams {
        stdout: stdout.map(|stream| capture_stream(stream, Arc::clone(&record), false)),
        stderr: stderr.map(|stream| capture_stream(stream, Arc::clone(&record), true)),
    };
    tokio::spawn(async move {
        let trigger = wait_for_trigger(&mut child, &mut stop, command_timeout).await;
        let completion = finish_child(&mut child, trigger).await;
        drain_streams(streams, &record, CAPTURE_DRAIN_TIMEOUT).await;
        finish_record(&manager, &record, completion);
    })
}

pub(super) enum Completion {
    Exited(ExitStatus),
    PtyExited { exit_code: i32, success: bool },
    TimedOut(Option<String>),
    Stopped(StopReason, Option<String>),
    Failed(String),
}

enum Trigger {
    Exited(io::Result<ExitStatus>),
    TimedOut,
    Stopped(StopReason),
}

async fn wait_for_trigger(
    child: &mut AsyncGroupChild,
    stop: &mut watch::Receiver<Option<StopReason>>,
    command_timeout: Option<Duration>,
) -> Trigger {
    let deadline = command_timeout.map(|duration| Instant::now() + duration);
    tokio::select! {
        result = child.inner().wait() => Trigger::Exited(result),
        () = wait_deadline(deadline) => Trigger::TimedOut,
        result = stop.changed() => {
            let reason = result.ok().and_then(|()| *stop.borrow()).unwrap_or(StopReason::Shutdown);
            Trigger::Stopped(reason)
        }
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

async fn finish_child(child: &mut AsyncGroupChild, trigger: Trigger) -> Completion {
    match trigger {
        Trigger::Exited(Err(error)) => Completion::Failed(error.to_string()),
        Trigger::Exited(Ok(status)) => match cleanup_after_exit(child).await {
            Some(error) => Completion::Failed(format!(
                "command exited but process-group cleanup failed: {error}"
            )),
            None => Completion::Exited(status),
        },
        Trigger::TimedOut => Completion::TimedOut(kill_and_reap(child).await),
        Trigger::Stopped(reason) => Completion::Stopped(reason, kill_and_reap(child).await),
    }
}

async fn cleanup_after_exit(child: &mut AsyncGroupChild) -> Option<String> {
    let kill_error = child
        .start_kill()
        .err()
        .filter(|error| !process_group_already_gone(error))
        .map(|error| error.to_string());
    let wait_error = reap(child).await;
    combine_cleanup_errors(kill_error, wait_error)
}

fn process_group_already_gone(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
    ) {
        return true;
    }
    // Tokio currently preserves Unix ESRCH as an uncategorized raw errno after the leader was
    // reaped. In that state there is no remaining process group to kill, but `wait` still runs.
    #[cfg(unix)]
    if error.raw_os_error() == Some(3) {
        return true;
    }
    false
}

async fn kill_and_reap(child: &mut AsyncGroupChild) -> Option<String> {
    let running = child.id().is_some();
    let kill_error = child
        .start_kill()
        .err()
        .filter(|error| running && error.kind() != io::ErrorKind::InvalidInput)
        .map(|error| error.to_string());
    let wait_error = reap(child).await;
    combine_cleanup_errors(kill_error, wait_error)
}

async fn reap(child: &mut AsyncGroupChild) -> Option<String> {
    match timeout(PROCESS_REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some(format!(
            "reap timed out after {} seconds",
            PROCESS_REAP_TIMEOUT.as_secs()
        )),
    }
}

fn combine_cleanup_errors(
    kill_error: Option<String>,
    wait_error: Option<String>,
) -> Option<String> {
    match (kill_error, wait_error) {
        (Some(kill), Some(wait)) => Some(format!("kill failed: {kill}; reap failed: {wait}")),
        (Some(kill), None) => Some(format!("kill failed: {kill}")),
        (None, Some(wait)) => Some(format!("reap failed: {wait}")),
        (None, None) => None,
    }
}

pub(super) fn capture_stream<R>(
    mut stream: R,
    record: Arc<ActivityRecord>,
    stderr: bool,
) -> JoinHandle<io::Result<()>>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        let mut bytes = [0_u8; 8 * 1024];
        loop {
            let read = stream.read(&mut bytes).await?;
            if read == 0 {
                return Ok(());
            }
            {
                let mut state = record
                    .state
                    .lock()
                    .expect("activity state mutex must not be poisoned");
                let stream = if stderr {
                    ActivityOutputStream::Stderr
                } else {
                    ActivityOutputStream::Stdout
                };
                state.output.append(stream, &bytes[..read]);
            }
            record.changed.notify_waiters();
        }
    })
}

pub(super) async fn drain_streams(
    streams: SpawnedStreams,
    record: &ActivityRecord,
    drain_timeout: Duration,
) {
    let mut stdout = streams.stdout;
    let mut stderr = streams.stderr;
    let mut stdout_incomplete = false;
    let mut stderr_incomplete = false;
    let drained = timeout(drain_timeout, async {
        if let Some(task) = stdout.as_mut() {
            stdout_incomplete = !matches!(task.await, Ok(Ok(())));
        }
        if let Some(task) = stderr.as_mut() {
            stderr_incomplete = !matches!(task.await, Ok(Ok(())));
        }
    })
    .await
    .is_ok();
    if !drained {
        if let Some(task) = stdout.as_ref() {
            task.abort();
        }
        if let Some(task) = stderr.as_ref() {
            task.abort();
        }
        // Aborting is only a cancellation request. A capture task can already be between its
        // final read and append, so joining it is what closes the output-mutation window before
        // `finish_record` snapshots the terminal payload.
        if let Some(task) = stdout.take() {
            let _ = task.await;
        }
        if let Some(task) = stderr.take() {
            let _ = task.await;
        }
        stdout_incomplete = true;
        stderr_incomplete = true;
    }
    if stdout_incomplete || stderr_incomplete {
        let mut state = record
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        if stdout_incomplete {
            state.output.mark_incomplete(ActivityOutputStream::Stdout);
        }
        if stderr_incomplete {
            state.output.mark_incomplete(ActivityOutputStream::Stderr);
        }
    }
}

pub(super) fn finish_record(
    manager: &Weak<ActivityManagerInner>,
    record: &Arc<ActivityRecord>,
    completion: Completion,
) {
    let Some(manager) = manager.upgrade() else {
        let mut state = record
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        apply_completion(&mut state, completion);
        drop(state);
        record.changed.notify_waiters();
        return;
    };
    let published = {
        let mut records = manager
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        let mut state = record
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        apply_completion(&mut state, completion);
        record.release_permit();
        let published = state.published;
        if !published {
            records.remove(&record.id);
        } else if let Some(output) = record.take_terminal_output(&mut state) {
            let _ = manager.events.send(ActivityEvent::Finished(output));
        }
        published
    };
    record.changed.notify_waiters();
    if published {
        retain_recent(&manager, record.id);
    }
}

fn apply_completion(state: &mut super::ActivityState, completion: Completion) {
    match completion {
        Completion::Exited(status) => {
            state.exit_code = status.code();
            state.status = if status.success() {
                ActivityStatus::Completed
            } else {
                ActivityStatus::Failed
            };
        }
        Completion::PtyExited { exit_code, success } => {
            state.exit_code = Some(exit_code);
            state.status = if success {
                ActivityStatus::Completed
            } else {
                ActivityStatus::Failed
            };
        }
        Completion::TimedOut(message) => {
            state.status = ActivityStatus::Stopped;
            state.message = Some(message.map_or_else(
                || "command timed out".to_owned(),
                |error| format!("command timed out; cleanup failed: {error}"),
            ));
        }
        Completion::Stopped(reason, message) => {
            state.status = ActivityStatus::Stopped;
            state.message = Some(message.unwrap_or_else(|| match reason {
                StopReason::Requested => "command stopped".to_owned(),
                StopReason::Shutdown => "command stopped during shutdown".to_owned(),
            }));
        }
        Completion::Failed(message) => {
            state.status = ActivityStatus::Failed;
            state.message = Some(message);
        }
    }
}
