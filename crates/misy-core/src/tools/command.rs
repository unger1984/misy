//! Unified shell-command parsing, spawning, and bounded foreground yielding.

use crate::{ActivityOutput, ActivityStatus, ToolCall, ToolResult, activity::ActivityOwner};
use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::Value;
use std::{io, process::Stdio, time::Duration};
use tokio::process::{ChildStderr, ChildStdout, Command};

use super::activity::{ActivityManager, output::DEFAULT_MAX_OUTPUT_TOKENS};

pub(super) const DEFAULT_YIELD_TIME: Duration = Duration::from_secs(10);
const MIN_YIELD_TIME_MS: u64 = 250;
const MAX_YIELD_TIME_MS: u64 = 30_000;
const MAX_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
const MAX_DESCRIPTION_CHARS: usize = 80;

#[derive(Debug)]
pub(super) struct CommandRequest {
    command: String,
    pub(super) cwd: Option<String>,
    pub(super) description: String,
    pub(super) run_in_background: bool,
    pub(super) yield_after: Duration,
    pub(super) timeout: Option<Duration>,
    pub(super) max_output_tokens: usize,
    pub(super) tty: bool,
}

pub(super) enum SpawnedCommand {
    Pipe {
        child: AsyncGroupChild,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    },
    Pty(super::activity::pty::PtySpawn),
}

impl CommandRequest {
    pub(super) fn from_call(
        call: &ToolCall,
        timeout_override: Option<Duration>,
    ) -> Result<Self, String> {
        let command = call
            .arguments
            .get("cmd")
            .and_then(Value::as_str)
            .ok_or_else(|| "arguments.cmd must be a string".to_owned())?
            .to_owned();
        let yield_after = duration_millis(call, "yield_time_ms", DEFAULT_YIELD_TIME)?;
        validate_yield(yield_after)?;
        let timeout = command_timeout(call, timeout_override)?;
        let description = call
            .arguments
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map_or_else(|| command_preview(&command), ToOwned::to_owned);
        let max_output_tokens = call
            .arguments
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_OUTPUT_TOKENS, |tokens| {
                usize::try_from(tokens).unwrap_or(usize::MAX)
            });
        Ok(Self {
            command,
            cwd: call
                .arguments
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            description,
            run_in_background: call
                .arguments
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            yield_after,
            timeout,
            max_output_tokens,
            tty: call
                .arguments
                .get("tty")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub(super) fn spawn(&self) -> io::Result<SpawnedCommand> {
        if self.tty {
            return super::activity::pty::spawn(&self.command, self.cwd.as_deref())
                .map(SpawnedCommand::Pty);
        }
        let mut command = platform_shell(&self.command);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.group_spawn()?;
        let stdout = child.inner().stdout.take();
        let stderr = child.inner().stderr.take();
        Ok(SpawnedCommand::Pipe {
            child,
            stdout,
            stderr,
        })
    }

    pub(super) fn program_display(&self) -> &str {
        &self.command
    }
}

pub(super) async fn run(
    call: &ToolCall,
    owner: ActivityOwner,
    manager: &ActivityManager,
    timeout_override: Option<Duration>,
    cancellation: Option<tokio::sync::watch::Receiver<bool>>,
) -> ToolResult {
    let request = match CommandRequest::from_call(call, timeout_override) {
        Ok(request) => request,
        Err(message) => return ToolResult::error(&call.id, message),
    };
    let wait = if request.run_in_background {
        Duration::ZERO
    } else {
        request.yield_after
    };
    let max_output_tokens = request.max_output_tokens;
    let pending = match manager.start_for_owner(owner, &request) {
        Ok(pending) => pending,
        Err(message) => return spawn_error(call, &message),
    };
    let output = pending
        .wait_or_promote(wait, cancellation, max_output_tokens)
        .await;
    result(call, &output)
}

fn validate_yield(duration: Duration) -> Result<(), String> {
    let milliseconds = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    if (MIN_YIELD_TIME_MS..=MAX_YIELD_TIME_MS).contains(&milliseconds) {
        Ok(())
    } else {
        Err(format!(
            "arguments.yield_time_ms must be between {MIN_YIELD_TIME_MS} and {MAX_YIELD_TIME_MS}"
        ))
    }
}

fn command_timeout(
    call: &ToolCall,
    timeout_override: Option<Duration>,
) -> Result<Option<Duration>, String> {
    let Some(value) = call.arguments.get("timeout_seconds") else {
        return Ok(timeout_override);
    };
    let seconds = value
        .as_u64()
        .ok_or_else(|| "arguments.timeout_seconds must be an integer".to_owned())?;
    if seconds > MAX_TIMEOUT_SECONDS {
        return Err(format!(
            "arguments.timeout_seconds must not exceed {MAX_TIMEOUT_SECONDS}"
        ));
    }
    Ok((seconds > 0).then(|| Duration::from_secs(seconds)))
}

fn command_preview(command: &str) -> String {
    let first_line = command.lines().next().unwrap_or_default().trim();
    let mut preview = first_line
        .chars()
        .take(MAX_DESCRIPTION_CHARS)
        .collect::<String>();
    if first_line.chars().count() > MAX_DESCRIPTION_CHARS {
        preview.push('…');
    }
    if preview.is_empty() {
        "shell command".to_owned()
    } else {
        preview
    }
}

#[cfg(unix)]
fn platform_shell(script: &str) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut command = Command::new(shell);
    command.arg("-lc").arg(script);
    command
}

#[cfg(windows)]
fn platform_shell(script: &str) -> Command {
    let mut command = Command::new("cmd.exe");
    command.args(["/D", "/S", "/C", script]);
    command
}

fn duration_millis(call: &ToolCall, name: &str, default: Duration) -> Result<Duration, String> {
    let Some(value) = call.arguments.get(name) else {
        return Ok(default);
    };
    let milliseconds = value
        .as_u64()
        .ok_or_else(|| format!("arguments.{name} must be an integer"))?;
    Ok(Duration::from_millis(milliseconds))
}

fn spawn_error(call: &ToolCall, message: &str) -> ToolResult {
    ToolResult::error(
        &call.id,
        serde_json::json!({
            "kind": "spawn_error",
            "exit_code": null,
            "stdout": "",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "message": message,
        })
        .to_string(),
    )
}

pub(super) fn result(call: &ToolCall, output: &ActivityOutput) -> ToolResult {
    let kind = match output.activity.status {
        ActivityStatus::Running => "background",
        ActivityStatus::Completed => "success",
        ActivityStatus::Failed if output.activity.exit_code.is_some() => "nonzero_exit",
        ActivityStatus::Failed => "wait_error",
        ActivityStatus::Stopped
            if output
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("command timed out")) =>
        {
            "timeout"
        }
        ActivityStatus::Stopped => "stopped",
        ActivityStatus::Queued | ActivityStatus::Waiting => "background",
    };
    let background = kind == "background";
    let content = serde_json::json!({
        "kind": kind,
        "task_id": background.then(|| output.activity.id.to_string()),
        "status": output.activity.status,
        "exit_code": output.activity.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
        "stdout_truncated": output.stdout_truncated,
        "stderr_truncated": output.stderr_truncated,
        "message": output.message,
        "poll_hint": background.then_some(
            "Poll with write_stdin using this task_id and empty chars until exit_code is returned."
        ),
    })
    .to_string();
    if matches!(kind, "success" | "background") {
        ToolResult::success(&call.id, content)
    } else {
        ToolResult::error(&call.id, content)
    }
}
