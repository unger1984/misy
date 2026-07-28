//! Asynchronous command execution and bounded output capture.

use crate::{ToolCall, ToolResult};
use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::Value;
use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time::timeout,
};

pub(super) const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
// A daemonized grandchild can keep the pipe's write end open after the process
// group is killed, so capture EOF is not guaranteed once the command finished.
// Normal EOF arrives instantly; this only bounds the pathological case.
const CAPTURE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn run(call: &ToolCall, command_timeout: Duration) -> ToolResult {
    let Some(command_name) = call.arguments.get("command").and_then(Value::as_str) else {
        return ToolResult::error(&call.id, "arguments.command must be a string");
    };
    let mut command = Command::new(command_name);
    if let Some(arguments) = call.arguments.get("args").and_then(Value::as_array) {
        for argument in arguments {
            let Some(argument) = argument.as_str() else {
                return ToolResult::error(&call.id, "arguments.args must contain strings");
            };
            command.arg(argument);
        }
    }
    if let Some(working_directory) = call.arguments.get("cwd").and_then(Value::as_str) {
        command.current_dir(working_directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.group_spawn() {
        Ok(child) => child,
        Err(error) => return spawn_error(call, command_name, &error),
    };
    let stdout = child.inner().stdout.take().map(capture_stream);
    let stderr = child.inner().stderr.take().map(capture_stream);
    let (completion_sender, completion_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // The receiver lives in `run`; a failed send means the caller went away.
        let _ = completion_sender.send(wait_for_command(child, command_timeout).await);
    });
    let completion = match completion_receiver.await {
        Ok(completion) => completion,
        Err(error) => return supervisor_error(call, command_name, stdout, stderr, error).await,
    };
    let (kind, status, message) = completion.parts();
    let stdout = join_capture(stdout).await;
    let stderr = join_capture(stderr).await;
    let kind = if stdout.truncated || stderr.truncated {
        "truncated"
    } else {
        kind
    };
    command_result(call, kind, status, &stdout, &stderr, message.as_deref())
}

fn spawn_error(call: &ToolCall, command_name: &str, error: &io::Error) -> ToolResult {
    command_result(
        call,
        "spawn_error",
        None,
        &CapturedStream::default(),
        &CapturedStream::default(),
        Some(&format!("could not run {command_name}: {error}")),
    )
}

async fn supervisor_error(
    call: &ToolCall,
    command_name: &str,
    stdout: Option<JoinHandle<io::Result<CapturedStream>>>,
    stderr: Option<JoinHandle<io::Result<CapturedStream>>>,
    error: tokio::sync::oneshot::error::RecvError,
) -> ToolResult {
    command_result(
        call,
        "wait_error",
        None,
        &join_capture(stdout).await,
        &join_capture(stderr).await,
        Some(&format!(
            "could not wait for {command_name}: supervisor stopped: {error}"
        )),
    )
}

#[derive(Default)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

fn capture_stream<R>(mut reader: R) -> JoinHandle<io::Result<CapturedStream>>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        let mut captured = CapturedStream::default();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(captured);
            }
            let available = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(captured.bytes.len());
            let kept = available.min(read);
            captured.bytes.extend_from_slice(&buffer[..kept]);
            captured.truncated |= kept < read;
        }
    })
}

async fn join_capture(handle: Option<JoinHandle<io::Result<CapturedStream>>>) -> CapturedStream {
    let Some(mut handle) = handle else {
        return CapturedStream::default();
    };
    match timeout(CAPTURE_DRAIN_TIMEOUT, &mut handle).await {
        Ok(joined) => joined
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(capture_failed),
        Err(_) => {
            handle.abort();
            CapturedStream::default()
        }
    }
}

// Silently swapping a failed capture for an empty stream would mask the failure
// as "the command printed nothing", so surface it through the same truncated
// marker the output budget uses.
fn capture_failed() -> CapturedStream {
    CapturedStream {
        bytes: Vec::new(),
        truncated: true,
    }
}

enum CommandCompletion {
    Completed(ExitStatus),
    TimedOut(Option<String>),
    WaitError(String),
}

impl CommandCompletion {
    fn parts(self) -> (&'static str, Option<ExitStatus>, Option<String>) {
        match self {
            Self::Completed(status) => {
                let kind = if status.success() {
                    "success"
                } else {
                    "nonzero_exit"
                };
                (kind, Some(status), None)
            }
            Self::TimedOut(message) => ("timeout", None, message),
            Self::WaitError(message) => ("wait_error", None, Some(message)),
        }
    }
}

async fn wait_for_command(
    mut child: AsyncGroupChild,
    command_timeout: Duration,
) -> CommandCompletion {
    // Elapsed carries no payload: None maps to TimedOut and Some(Err) to WaitError below.
    let leader_result = timeout(command_timeout, child.inner().wait()).await.ok();
    let leader_is_running = child.id().is_some();
    let kill_error = child
        .start_kill()
        .err()
        .filter(|error| leader_is_running && error.kind() != io::ErrorKind::InvalidInput)
        .map(|error| error.to_string());
    let group_result = child.wait().await;
    match leader_result {
        None => CommandCompletion::TimedOut(
            kill_error.or_else(|| group_result.err().map(|error| error.to_string())),
        ),
        Some(Err(error)) => CommandCompletion::WaitError(error.to_string()),
        Some(Ok(_)) => {
            if let Some(error) = kill_error {
                CommandCompletion::WaitError(error)
            } else {
                match group_result {
                    Ok(status) => CommandCompletion::Completed(status),
                    Err(error) => CommandCompletion::WaitError(error.to_string()),
                }
            }
        }
    }
}

fn command_result(
    call: &ToolCall,
    kind: &str,
    status: Option<ExitStatus>,
    stdout: &CapturedStream,
    stderr: &CapturedStream,
    message: Option<&str>,
) -> ToolResult {
    let content = serde_json::json!({
        "kind": kind,
        "exit_code": status.and_then(|status| status.code()),
        "stdout": String::from_utf8_lossy(&stdout.bytes),
        "stderr": String::from_utf8_lossy(&stderr.bytes),
        "stdout_truncated": stdout.truncated,
        "stderr_truncated": stderr.truncated,
        "message": message,
    })
    .to_string();
    if kind == "success" {
        ToolResult::success(&call.id, content)
    } else {
        ToolResult::error(&call.id, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn marks_output_truncated_when_the_capture_task_fails() {
        let handle: JoinHandle<io::Result<CapturedStream>> =
            tokio::spawn(async { Err(io::Error::new(io::ErrorKind::BrokenPipe, "read failed")) });

        let captured = join_capture(Some(handle)).await;

        assert!(captured.truncated);
        assert!(captured.bytes.is_empty());
    }

    #[tokio::test]
    async fn marks_output_truncated_when_the_capture_task_panics() {
        let handle: JoinHandle<io::Result<CapturedStream>> =
            tokio::spawn(async { panic!("capture task blew up") });

        let captured = join_capture(Some(handle)).await;

        assert!(captured.truncated);
        assert!(captured.bytes.is_empty());
    }

    #[tokio::test]
    async fn keeps_a_successful_empty_capture_clean() {
        let handle: JoinHandle<io::Result<CapturedStream>> =
            tokio::spawn(async { Ok(CapturedStream::default()) });

        let captured = join_capture(Some(handle)).await;

        assert!(!captured.truncated);
        assert!(captured.bytes.is_empty());
    }
}
