//! Schema validation, instruction-scope activation, and normalized tool-call preparation.

use super::{AgentTurnIdentity, AgentTurnState};
use crate::{
    CoreEvent, ToolCall, ToolResult,
    activity::ActivityOwner,
    core::{CoreState, instruction_paths::resolve_target, instructions::InstructionRoot},
    tools::ToolScopePolicy,
};
use serde_json::{Value, json};

pub(super) struct PreparedBatch {
    pub(super) calls: Vec<Option<ToolCall>>,
    pub(super) results: Vec<ToolResult>,
    pub(super) halted: bool,
}

pub(super) fn prepare_tool_batch(
    core: &CoreState,
    state: &AgentTurnState,
    calls: &[ToolCall],
) -> PreparedBatch {
    let root = state
        .instructions()
        .lock()
        .expect("instruction session mutex must not be poisoned")
        .root();
    let owner = match state.identity() {
        AgentTurnIdentity::Main => ActivityOwner::Main,
        AgentTurnIdentity::Child(id) => ActivityOwner::Agent(id),
    };
    let mut prepared = Vec::with_capacity(calls.len());
    let mut final_results = Vec::new();
    let mut targets = Vec::new();
    for call in calls {
        if let Err(error) = core.dispatcher.validate_arguments(call) {
            final_results.push(ToolResult::error(&call.id, error.to_string()));
            prepared.push(None);
            continue;
        }
        match prepare_tool_call(core, &root, owner, call) {
            Ok((call, target)) => {
                if let Some(target) = target {
                    targets.push(target);
                }
                prepared.push(Some(call));
            }
            Err(message) => {
                final_results.push(ToolResult::error(&call.id, message));
                prepared.push(None);
            }
        }
    }
    let discovery = state
        .instructions()
        .lock()
        .expect("instruction session mutex must not be poisoned")
        .discover(&targets);
    for warning in discovery.warnings {
        core.emit(&CoreEvent::InstructionWarning { warning });
    }
    let halt = discovery.error.map_or_else(
        || {
            discovery.activated.then(|| {
                json!({
                    "kind": "instruction_scope_retry_required",
                    "message": "new AGENTS.md scopes were activated; retry this tool call",
                })
                .to_string()
            })
        },
        |message| {
            Some(json!({"kind": "instruction_scope_blocked", "message": message}).to_string())
        },
    );
    let Some(message) = halt else {
        return PreparedBatch {
            calls: prepared,
            results: final_results,
            halted: false,
        };
    };
    let mut results = Vec::with_capacity(calls.len());
    let mut finals = final_results.into_iter();
    for (call, prepared) in calls.iter().zip(&prepared) {
        if prepared.is_some() {
            results.push(ToolResult::error(&call.id, message.clone()));
        } else {
            results.push(
                finals
                    .next()
                    .expect("every unprepared tool call must have a final result"),
            );
        }
    }
    for result in &results {
        super::r#loop::emit_tool_result(core, state, result.clone());
    }
    PreparedBatch {
        calls: prepared,
        results,
        halted: true,
    }
}

pub(super) fn prepared_result(results: &[ToolResult], call: &ToolCall) -> ToolResult {
    results
        .iter()
        .find(|result| result.tool_call_id == call.id)
        .cloned()
        .unwrap_or_else(|| ToolResult::error(&call.id, "tool preparation failed"))
}

fn prepare_tool_call(
    core: &CoreState,
    root: &InstructionRoot,
    owner: ActivityOwner,
    call: &ToolCall,
) -> Result<
    (
        ToolCall,
        Option<crate::core::instruction_paths::ResolvedTarget>,
    ),
    String,
> {
    let policy = core
        .dispatcher
        .scope_policy(&call.name)
        .map_err(|error| error.to_string())?;
    let raw = match policy {
        ToolScopePolicy::PathArgument(argument) => call
            .arguments
            .get(argument)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        ToolScopePolicy::WorkingDirectory(argument) => Some(
            call.arguments
                .get(argument)
                .and_then(Value::as_str)
                .map_or_else(
                    || root.workspace_cwd().display().to_string(),
                    ToOwned::to_owned,
                ),
        ),
        ToolScopePolicy::ActivityWorkingDirectory(_argument) => {
            core.dispatcher.activity_cwd(call, owner)
        }
        ToolScopePolicy::None => None,
    };
    let Some(raw) = raw else {
        return Ok((call.clone(), None));
    };
    let target = resolve_target(root.workspace_cwd(), &raw)?;
    let mut normalized = call.clone();
    match policy {
        ToolScopePolicy::PathArgument(argument) | ToolScopePolicy::WorkingDirectory(argument) => {
            normalized.arguments[argument] = Value::String(target.path.display().to_string());
        }
        ToolScopePolicy::ActivityWorkingDirectory(_) | ToolScopePolicy::None => {}
    }
    Ok((normalized, Some(target)))
}
