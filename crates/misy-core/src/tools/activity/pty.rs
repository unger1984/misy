//! Unix pseudo-terminal spawning, input ownership, and process-group cleanup.

use super::{
    ActivityManagerInner, ActivityRecord, StopReason,
    process::{self, Completion},
};
use crate::ActivityOutputStream;
use portable_pty::{Child, ChildKiller, CommandBuilder, ExitStatus, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout},
};

const TERMINATE_GRACE: Duration = Duration::from_millis(500);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::tools) struct PtySpawn {
    child: Box<dyn Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    process_group: Option<i32>,
}

#[cfg(unix)]
pub(in crate::tools) fn spawn(command: &str, cwd: Option<&str>) -> io::Result<PtySpawn> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(pty_error)?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut builder = CommandBuilder::new(shell);
    builder.arg("-lc");
    builder.arg(command);
    if let Some(cwd) = cwd {
        builder.cwd(cwd);
    }
    let child = pair.slave.spawn_command(builder).map_err(pty_error)?;
    let process_group = pair.master.process_group_leader();
    let reader = pair.master.try_clone_reader().map_err(pty_error)?;
    let writer = pair.master.take_writer().map_err(pty_error)?;
    Ok(PtySpawn {
        child,
        reader,
        writer,
        process_group,
    })
}

#[cfg(not(unix))]
pub(in crate::tools) fn spawn(_command: &str, _cwd: Option<&str>) -> io::Result<PtySpawn> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "tty mode is unsupported on Windows; ConPTY and Job Object support is deferred",
    ))
}

fn pty_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub(super) fn spawn_supervisor(
    manager: Weak<ActivityManagerInner>,
    record: Arc<ActivityRecord>,
    spawned: PtySpawn,
    mut stop: watch::Receiver<Option<StopReason>>,
    command_timeout: Option<Duration>,
) -> JoinHandle<()> {
    let PtySpawn {
        child,
        reader,
        writer,
        process_group,
    } = spawned;
    record.install_pty_writer(writer);
    let capture = spawn_capture(reader, Arc::clone(&record));
    let mut killer = child.clone_killer();
    let mut wait = tokio::task::spawn_blocking(move || {
        let mut child = child;
        child.wait()
    });
    tokio::spawn(async move {
        let trigger = wait_for_trigger(&mut wait, &mut stop, command_timeout).await;
        let completion =
            finish_process(trigger, &mut wait, &mut killer, process_group, &record).await;
        drain_capture(capture, &record).await;
        process::finish_record(&manager, &record, completion);
    })
}

enum Trigger {
    Exited(Result<io::Result<ExitStatus>, tokio::task::JoinError>),
    TimedOut,
    Stopped(StopReason),
}

async fn wait_for_trigger(
    wait: &mut JoinHandle<io::Result<ExitStatus>>,
    stop: &mut watch::Receiver<Option<StopReason>>,
    command_timeout: Option<Duration>,
) -> Trigger {
    let deadline = command_timeout.map(|duration| Instant::now() + duration);
    tokio::select! {
        result = wait => Trigger::Exited(result),
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

async fn finish_process(
    trigger: Trigger,
    wait: &mut JoinHandle<io::Result<ExitStatus>>,
    killer: &mut Box<dyn ChildKiller + Send + Sync>,
    process_group: Option<i32>,
    record: &ActivityRecord,
) -> Completion {
    let completion = match trigger {
        Trigger::Exited(result) => {
            cleanup_remaining_group(process_group).await;
            completion_from_wait(result)
        }
        Trigger::TimedOut => {
            record.close_input();
            let cleanup = terminate_and_reap(wait, killer, process_group).await;
            Completion::TimedOut(cleanup)
        }
        Trigger::Stopped(reason) => {
            record.close_input();
            let cleanup = terminate_and_reap(wait, killer, process_group).await;
            Completion::Stopped(reason, cleanup)
        }
    };
    record.close_input();
    completion
}

async fn terminate_and_reap(
    wait: &mut JoinHandle<io::Result<ExitStatus>>,
    killer: &mut Box<dyn ChildKiller + Send + Sync>,
    process_group: Option<i32>,
) -> Option<String> {
    let term_error = signal_group(process_group, false).err();
    sleep(TERMINATE_GRACE).await;
    // The leader can exit before a descendant that ignored TERM. Escalating the group after the
    // full grace window keeps cleanup tied to the group lifecycle rather than only leader reap.
    let kill_error = signal_group(process_group, true).err();
    let child_error = if wait.is_finished() {
        None
    } else {
        killer.kill().err().map(|error| error.to_string())
    };
    let reap_error = match timeout(REAP_TIMEOUT, &mut *wait).await {
        Ok(result) => completion_from_wait(result).failure_message(),
        Err(_) => Some(format!(
            "reap timed out after {} seconds",
            REAP_TIMEOUT.as_secs()
        )),
    };
    join_errors([term_error, kill_error, child_error, reap_error])
}

async fn cleanup_remaining_group(process_group: Option<i32>) {
    if process_group.is_some() && signal_group(process_group, false).is_ok() {
        sleep(TERMINATE_GRACE).await;
        let _ = signal_group(process_group, true);
    }
}

#[cfg(unix)]
fn signal_group(process_group: Option<i32>, hard: bool) -> Result<(), String> {
    let Some(process_group) = process_group else {
        return Ok(());
    };
    let signal = if hard {
        nix::sys::signal::Signal::SIGKILL
    } else {
        nix::sys::signal::Signal::SIGTERM
    };
    nix::sys::signal::killpg(nix::unistd::Pid::from_raw(process_group), signal).map_err(|error| {
        if error == nix::errno::Errno::ESRCH {
            String::new()
        } else {
            error.to_string()
        }
    })
}

#[cfg(not(unix))]
fn signal_group(_process_group: Option<i32>, _hard: bool) -> Result<(), String> {
    Ok(())
}

fn completion_from_wait(
    result: Result<io::Result<ExitStatus>, tokio::task::JoinError>,
) -> Completion {
    match result {
        Ok(Ok(status)) => Completion::PtyExited {
            exit_code: i32::try_from(status.exit_code()).unwrap_or(i32::MAX),
            success: status.success(),
        },
        Ok(Err(error)) => Completion::Failed(error.to_string()),
        Err(error) => Completion::Failed(format!("PTY wait task failed: {error}")),
    }
}

fn spawn_capture(reader: Box<dyn Read + Send>, record: Arc<ActivityRecord>) -> JoinHandle<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || read_chunks(reader, &sender));
    tokio::spawn(async move {
        while let Some(bytes) = receiver.recv().await {
            record
                .state
                .lock()
                .expect("activity state mutex must not be poisoned")
                .output
                .append(ActivityOutputStream::Stdout, &bytes);
            record.changed.notify_waiters();
        }
    })
}

fn read_chunks(mut reader: Box<dyn Read + Send>, sender: &mpsc::UnboundedSender<Vec<u8>>) {
    let mut bytes = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(read) if sender.send(bytes[..read].to_vec()).is_err() => return,
            Ok(_) => {}
        }
    }
}

async fn drain_capture(capture: JoinHandle<()>, record: &ActivityRecord) {
    let mut capture = capture;
    if timeout(process::CAPTURE_DRAIN_TIMEOUT, &mut capture)
        .await
        .is_err()
    {
        capture.abort();
        let _ = capture.await;
        record
            .state
            .lock()
            .expect("activity state mutex must not be poisoned")
            .output
            .mark_incomplete(ActivityOutputStream::Stdout);
    }
}

fn join_errors<const N: usize>(errors: [Option<String>; N]) -> Option<String> {
    let messages = errors
        .into_iter()
        .flatten()
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>();
    (!messages.is_empty()).then(|| messages.join("; "))
}

trait CompletionFailure {
    fn failure_message(self) -> Option<String>;
}

impl CompletionFailure for Completion {
    fn failure_message(self) -> Option<String> {
        match self {
            Completion::Failed(message) => Some(message),
            _ => None,
        }
    }
}
