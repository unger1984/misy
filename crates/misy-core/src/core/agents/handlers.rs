//! Runtime handlers for main-session child-agent tools.

use super::{AgentId, AgentRecord, InboxDecision, MailboxWait, normalize_agent_title};
use crate::{
    ActivityStatus, CoreError, CoreEvent, HistoryEntry, Message, ModelRef, ToolCall, ToolResult,
    activity::ActivityOwner,
    core::{CoreState, turn},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

const DEFAULT_WAIT_MS: u64 = 10_000;
const SYNC_AGENT_DEADLINE: Duration = Duration::from_secs(30 * 60);
const AGENT_CLEANUP_DEADLINE: Duration = Duration::from_secs(5);
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    task: String,
    description: Option<String>,
    #[serde(default)]
    run_in_background: bool,
    model: Option<ModelRef>,
}

pub(crate) fn dispatch_agent_tool<'a>(
    core: &'a CoreState,
    parent: &'a turn::AgentTurnState,
    call: &'a ToolCall,
) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
    Box::pin(async move {
        if let Err(error) = core.dispatcher.validate_arguments(call) {
            return ToolResult::error(&call.id, error.to_string());
        }
        let result = match call.name.as_str() {
            "spawn_agent" => spawn(core, parent, call).await,
            "agent_list" => encode(core.agents.list()),
            "agent_wait" => wait(core, parent, call).await,
            "agent_output" => output(core, call),
            "agent_message" => message(core, call),
            "agent_stop" => stop(core, call),
            _ => Err(format!("tool `{}` is not an agent tool", call.name)),
        };
        match result {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(message) => ToolResult::error(&call.id, message),
        }
    })
}

async fn spawn(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let args: SpawnArgs = serde_json::from_value(call.arguments.clone())
        .map_err(|error| format!("invalid spawn_agent arguments: {error}"))?;
    if args.task.trim().is_empty() {
        return Err("arguments.task must not be empty".to_owned());
    }
    let model = args.model.unwrap_or_else(|| parent.model().clone());
    validate_model(core, &model).map_err(|error| error.to_string())?;
    let prefix = turn::forkable_prefix(
        &parent
            .history()
            .lock()
            .expect("parent history mutex must not be poisoned"),
    );
    let history = turn::child_history(&prefix, &args.task);
    let activity_id = core.dispatcher.next_activity_id();
    let title = normalize_agent_title(args.description.as_deref(), &args.task);
    let record = core
        .agents
        .register(
            activity_id,
            title,
            model.clone(),
            &args.task,
            args.run_in_background,
        )
        .map_err(|error| error.to_string())?;
    core.emit(&CoreEvent::ActivityChanged {
        activity: record.summary().activity_summary(),
    });
    let child = turn::AgentTurnState::child(record.summary().id, model, history);
    record.attach_active(Arc::clone(child.active()));
    let Some(core) = core.weak_self().upgrade() else {
        return Err(CoreError::Shutdown.to_string());
    };
    tokio::spawn(run_child(core, Arc::clone(&record), child));
    if args.run_in_background {
        return encode(json!({"agent_id": record.summary().id, "status": "running"}));
    }
    wait_for_sync(parent, &record).await
}

fn validate_model(core: &CoreState, model: &ModelRef) -> Result<(), CoreError> {
    let cached = core.model_cache.load();
    if !cached.iter().any(|candidate| &candidate.model == model) {
        return Err(CoreError::AgentModelUnavailable(model.clone()));
    }
    if !core.credential_epoch(&model.provider).present {
        return Err(CoreError::AgentAuthenticationRequired(
            model.provider.clone(),
        ));
    }
    Ok(())
}

async fn wait_for_sync(
    parent: &turn::AgentTurnState,
    record: &Arc<AgentRecord>,
) -> Result<String, String> {
    let mut cancellation = parent.active().cancellation_receiver();
    let completed = tokio::select! {
        completed = record.wait_terminal(SYNC_AGENT_DEADLINE) => completed,
        _ = wait_for_cancellation(&mut cancellation) => {
            record.cancel();
            record.wait_terminal(AGENT_CLEANUP_DEADLINE).await
        }
    };
    if !completed {
        record.cancel();
        let _ = record.wait_terminal(AGENT_CLEANUP_DEADLINE).await;
        return Err(CoreError::AgentTimedOut(record.summary().id).to_string());
    }
    let summary = record.summary();
    let result = record.final_result().unwrap_or_default();
    let content = encode(json!({
        "agent_id": summary.id,
        "status": status_name(summary.status),
        "result": result,
    }))?;
    if summary.status == ActivityStatus::Completed {
        Ok(content)
    } else {
        Err(content)
    }
}

async fn run_child(core: Arc<CoreState>, record: Arc<AgentRecord>, child: turn::AgentTurnState) {
    let mut remaining_turns = turn::MAX_MODEL_TURNS;
    let mut captured = child
        .history()
        .lock()
        .expect("child history mutex must not be poisoned")
        .len();
    let (status, result) = loop {
        let outcome = turn::run_turns_with_limit(&core, &child, &mut remaining_turns).await;
        let history = child
            .history()
            .lock()
            .expect("child history mutex must not be poisoned")
            .clone();
        record.capture_history(&history[captured..]);
        match outcome {
            Ok(()) => match record.decide_inbox(remaining_turns > 0) {
                InboxDecision::Continue(messages) => {
                    append_messages(&child, messages);
                    captured = child
                        .history()
                        .lock()
                        .expect("child history mutex must not be poisoned")
                        .len();
                }
                InboxDecision::Close => {
                    break (ActivityStatus::Completed, final_assistant(&history));
                }
            },
            Err(error)
                if child
                    .active()
                    .cancelled
                    .load(std::sync::atomic::Ordering::Acquire) =>
            {
                break (ActivityStatus::Stopped, error);
            }
            Err(error) => break (ActivityStatus::Failed, error),
        }
    };
    let id = record.summary().id;
    core.dispatcher
        .stop_owner_commands(ActivityOwner::Agent(id))
        .await;
    if core.agents.finish(&record, status, &result) {
        let summary = record.summary();
        core.emit(&CoreEvent::ActivityChanged {
            activity: summary.activity_summary(),
        });
        if summary.run_in_background {
            core.emit(&CoreEvent::AgentFinished {
                agent: summary,
                result,
            });
        }
    }
}

fn append_messages(child: &turn::AgentTurnState, messages: Vec<String>) {
    let mut history = child
        .history()
        .lock()
        .expect("child history mutex must not be poisoned");
    history.extend(messages.into_iter().map(|message| HistoryEntry {
        message: Message::user(message),
        attachments: Vec::new(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        provider_metadata: Value::Null,
    }));
}

async fn wait(
    core: &CoreState,
    parent: &turn::AgentTurnState,
    call: &ToolCall,
) -> Result<String, String> {
    let ids: Option<Vec<AgentId>> = call
        .arguments
        .get("agent_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(parse_agent_value)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let timeout_ms = call
        .arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WAIT_MS);
    let outcome = core
        .agents
        .wait_mailbox(
            ids.as_deref(),
            Duration::from_millis(timeout_ms),
            parent.active().cancellation_receiver(),
        )
        .await
        .map_err(|error| error.to_string())?;
    match outcome {
        MailboxWait::Ready(agents) => encode(json!({"timed_out": false, "agents": agents})),
        MailboxWait::TimedOut => encode(json!({"timed_out": true, "agents": []})),
        MailboxWait::Cancelled => Err("agent_wait cancelled".to_owned()),
    }
}

fn output(core: &CoreState, call: &ToolCall) -> Result<String, String> {
    let id = parse_agent_argument(call)?;
    let transcript = core
        .agents
        .transcript(id)
        .map_err(|error| error.to_string())?;
    let maximum = call
        .arguments
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
        .saturating_mul(4);
    let text = transcript
        .entries
        .iter()
        .map(|entry| format!("{:?}: {}", entry.kind, entry.content))
        .collect::<Vec<_>>()
        .join("\n");
    encode(json!({
        "agent": transcript.agent,
        "transcript": project_text(&text, maximum),
        "truncated": transcript.truncated || text.len() > maximum,
    }))
}

fn message(core: &CoreState, call: &ToolCall) -> Result<String, String> {
    let id = parse_agent_argument(call)?;
    let message = call
        .arguments
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .ok_or_else(|| "arguments.message must not be empty".to_owned())?;
    core.agents
        .message(id, message)
        .map_err(|error| error.to_string())?;
    encode(json!({"agent_id": id, "status": "accepted"}))
}

fn stop(core: &CoreState, call: &ToolCall) -> Result<String, String> {
    let id = parse_agent_argument(call)?;
    core.agents.stop(id).map_err(|error| error.to_string())?;
    encode(json!({"agent_id": id, "status": "stopping"}))
}

fn parse_agent_argument(call: &ToolCall) -> Result<AgentId, String> {
    call.arguments
        .get("agent_id")
        .map(parse_agent_value)
        .transpose()?
        .ok_or_else(|| "arguments.agent_id is required".to_owned())
}

fn parse_agent_value(value: &Value) -> Result<AgentId, String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "agent_id must be a string".to_owned())?;
    let number = raw
        .strip_prefix("agent-")
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .filter(|number| *number > 0)
        .ok_or_else(|| format!("invalid agent id `{raw}`"))?;
    Ok(AgentId::new(number))
}

fn final_assistant(history: &[HistoryEntry]) -> String {
    history
        .iter()
        .rev()
        .find(|entry| entry.message.role == crate::MessageRole::Assistant)
        .map_or_else(String::new, |entry| entry.message.content.clone())
}

fn status_name(status: ActivityStatus) -> &'static str {
    match status {
        ActivityStatus::Completed => "completed",
        ActivityStatus::Failed => "failed",
        ActivityStatus::Stopped => "stopped",
        ActivityStatus::Queued => "queued",
        ActivityStatus::Running => "running",
        ActivityStatus::Waiting => "waiting",
    }
}

fn project_text(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_owned();
    }
    let half = maximum.saturating_sub(64) / 2;
    let head = text.chars().take(half).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{head}\n[... output omitted ...]\n{tail}")
}

async fn wait_for_cancellation(cancellation: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancellation.borrow() || cancellation.changed().await.is_err() {
            return;
        }
    }
}

fn encode(value: impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(&value).map_err(|error| format!("could not encode agent result: {error}"))
}
