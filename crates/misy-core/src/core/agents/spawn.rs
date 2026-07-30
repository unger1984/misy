//! Child-agent spawning, profile selection, fallback, and lifecycle execution.

use super::profiles::{
    ForkTurns, fork_prefix, intersect_tools, parse_fork_turns, profile_candidates, resolve_role,
    viable_profiles,
};
use super::{AgentRecord, AgentRegistration, InboxDecision, normalize_agent_title};
use crate::{
    ActivityStatus, CoreError, CoreEvent, HistoryEntry, InstructionOwner, Message, ModelProfile,
    ToolCall, ToolResult,
    activity::ActivityOwner,
    core::{CoreState, turn},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

const SYNC_AGENT_DEADLINE: Duration = Duration::from_secs(30 * 60);
const AGENT_CLEANUP_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    task: String,
    task_name: String,
    description: Option<String>,
    #[serde(default)]
    run_in_background: bool,
    agent_type: Option<String>,
    model: Option<SpawnModel>,
    #[serde(default = "default_fork_turns")]
    fork_turns: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum SpawnModel {
    Strict(String),
    Chain(Vec<String>),
}

fn default_fork_turns() -> String {
    "all".to_owned()
}

pub(crate) fn dispatch_agent_tool<'a>(
    core: &'a CoreState,
    parent: &'a turn::AgentTurnState,
    call: &'a ToolCall,
) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
    Box::pin(async move {
        let result = match call.name.as_str() {
            "spawn_agent" => spawn(core, parent, call).await,
            "agent_list" => encode(core.agents.list()),
            "agent_wait" => super::handlers::wait(core, parent, call).await,
            "agent_output" => super::handlers::output(core, parent, call),
            "agent_message" => super::handlers::message(core, parent, call),
            "agent_stop" => super::handlers::stop(core, parent, call),
            "model_search" => model_search(core, call).await,
            _ => Err(format!("tool `{}` is not an agent tool", call.name)),
        };
        match result {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(message) => ToolResult::error(&call.id, message),
        }
    })
}

async fn model_search(core: &CoreState, call: &ToolCall) -> Result<String, String> {
    let query = call.arguments.get("query").and_then(Value::as_str);
    let provider = call
        .arguments
        .get("provider")
        .and_then(Value::as_str)
        .map(crate::ProviderId::new);
    let agent_type = call.arguments.get("agent_type").and_then(Value::as_str);
    let limit = call
        .arguments
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(8);
    let matches = core
        .search_models(query, provider.as_ref(), agent_type, limit)
        .await
        .map_err(|error| error.to_string())?;
    encode(json!({"matches": matches}))
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
    let fork = parse_fork_turns(&args.fork_turns)?;
    if matches!(fork, ForkTurns::All) && (args.agent_type.is_some() || args.model.is_some()) {
        return Err(
            "agent_type/model overrides require fork_turns=`none` or a positive integer".to_owned(),
        );
    }
    let role = resolve_role(core, parent, args.agent_type.as_deref(), fork)?;
    let candidates = profile_candidates(parent, role.models.as_deref(), args.model)?;
    let profiles = viable_profiles(core, &candidates).await?;
    let profile = profiles
        .first()
        .expect("viable profile chain must not be empty")
        .clone();
    let prefix = turn::forkable_prefix(
        &parent
            .history()
            .lock()
            .expect("parent history mutex must not be poisoned"),
    );
    let forked = fork_prefix(&prefix, fork);
    let history = turn::child_history(&forked, &args.task);
    let activity_id = core.dispatcher.next_activity_id();
    let title = normalize_agent_title(args.description.as_deref(), &args.task);
    let record = core
        .agents
        .register(AgentRegistration {
            activity_id,
            title,
            profiles: profiles.clone(),
            parent: match parent.identity() {
                turn::AgentTurnIdentity::Main => None,
                turn::AgentTurnIdentity::Child(id) => Some(id),
            },
            task_name: args.task_name,
            role: role.name.clone(),
            task: args.task.clone(),
            run_in_background: args.run_in_background,
        })
        .map_err(|error| error.to_string())?;
    core.emit(&CoreEvent::ActivityChanged {
        activity: record.summary().activity_summary(),
    });
    let child_id = record.summary().id;
    let instruction_root = core
        .instructions
        .lock()
        .expect("instruction runtime mutex must not be poisoned")
        .root();
    let child_instructions = Arc::new(std::sync::Mutex::new(
        crate::core::instructions::InstructionSession::new(
            instruction_root,
            InstructionOwner::Child(child_id),
        ),
    ));
    let allowed_tools = intersect_tools(parent.allowed_tools(), role.tools.as_ref());
    let child = turn::AgentTurnState::child(
        child_id,
        profile,
        turn::ChildTurnConfig {
            role: Some(role.name),
            role_instructions: role.instructions,
            allowed_tools,
            history,
            instructions: child_instructions,
            role_catalog: super::super::roles::parent_catalog_description(&core.discover_roles()),
        },
    );
    child
        .instructions()
        .lock()
        .expect("instruction session mutex must not be poisoned")
        .begin_submission();
    record.attach_active(Arc::clone(child.active()));
    let Some(core) = core.weak_self().upgrade() else {
        return Err(CoreError::Shutdown.to_string());
    };
    tokio::spawn(run_child(core, Arc::clone(&record), child, profiles));
    if args.run_in_background {
        return encode(json!({"agent_id": record.summary().id, "status": "running"}));
    }
    wait_for_sync(parent, &record).await
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

async fn run_child(
    core: Arc<CoreState>,
    record: Arc<AgentRecord>,
    mut child: turn::AgentTurnState,
    profiles: Vec<ModelProfile>,
) {
    let mut remaining_turns = turn::MAX_MODEL_TURNS;
    let mut profile_index = 0;
    let mut overflow_retried = false;
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
        let outcome = if let Err(error) = &outcome
            && error.is_context_limit()
            && error.allows_fallback()
            && !overflow_retried
        {
            overflow_retried = true;
            match core.compact_agent_after_overflow(&child).await {
                Ok(()) => continue,
                Err(compaction) => Err(turn::TurnFailure::terminal(format!(
                    "overflow compaction failed: {compaction}"
                ))),
            }
        } else {
            outcome
        };
        match outcome {
            Ok(()) => match record.decide_inbox(remaining_turns > 0) {
                InboxDecision::Continue(messages) => {
                    append_messages(&child, messages);
                    let mut instructions = child
                        .instructions()
                        .lock()
                        .expect("instruction session mutex must not be poisoned");
                    instructions.restart_submission();
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
                if error.is_cancelled()
                    || child
                        .active()
                        .cancelled
                        .load(std::sync::atomic::Ordering::Acquire) =>
            {
                break (ActivityStatus::Stopped, error.to_string());
            }
            Err(error)
                if error.allows_fallback()
                    && !child.fallback_closed()
                    && profile_index + 1 < profiles.len() =>
            {
                profile_index += 1;
                let next = &profiles[profile_index];
                record.switch_attempt(&error.to_string(), next);
                child = child.with_profile(next);
                overflow_retried = false;
            }
            Err(error) if error.allows_fallback() && !child.fallback_closed() => {
                record.fail_attempt(&error.to_string());
                break (ActivityStatus::Failed, record.exhausted_profiles());
            }
            Err(error) => break (ActivityStatus::Failed, error.to_string()),
        }
    };
    finish_child(&core, &record, &child, status, result).await;
}

async fn finish_child(
    core: &CoreState,
    record: &AgentRecord,
    child: &turn::AgentTurnState,
    status: ActivityStatus,
    result: String,
) {
    let id = record.summary().id;
    core.dispatcher
        .stop_owner_commands(ActivityOwner::Agent(id))
        .await;
    // A parent agent owns the lifecycle of every descendant it created.
    let _ = core.agents.stop_descendants(id);
    child
        .instructions()
        .lock()
        .expect("instruction session mutex must not be poisoned")
        .finish_submission();
    let (_, instruction_sources, _) = child
        .instructions()
        .lock()
        .expect("instruction session mutex must not be poisoned")
        .report_sources();
    record.set_instruction_sources(instruction_sources);
    let (input_tokens, output_tokens) = child
        .history()
        .lock()
        .expect("child history mutex must not be poisoned")
        .iter()
        .rev()
        .find(|entry| entry.message.role == crate::MessageRole::Assistant)
        .map_or((None, None), |entry| {
            (
                entry
                    .provider_metadata
                    .get("input_tokens")
                    .and_then(Value::as_u64),
                entry
                    .provider_metadata
                    .get("output_tokens")
                    .and_then(Value::as_u64),
            )
        });
    record.complete_attempt(
        status == ActivityStatus::Completed,
        input_tokens,
        output_tokens,
    );
    if core.agents.finish(record, status, &result) {
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
