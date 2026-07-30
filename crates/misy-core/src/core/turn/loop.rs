//! Shared model/tool loop for one [`AgentTurnState`](super::AgentTurnState).

use super::super::CoreState;
use super::{AgentTurnState, PersistencePolicy, TurnEventSink, stream};
use crate::{
    CoreEvent, HistoryEntry, InputModality, Message, MessageRole, ToolCall, activity::ActivityOwner,
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(crate) const MAX_MODEL_TURNS: usize = 64;

/// Typed model-turn failure used to enforce the fallback safety boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TurnFailure {
    pub(super) kind: TurnFailureKind,
    message: String,
    unsafe_output: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TurnFailureKind {
    Cancelled,
    ContextLimit,
    RetryableProfile,
    Terminal,
}

impl TurnFailure {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.kind == TurnFailureKind::Cancelled
    }

    pub(crate) fn is_context_limit(&self) -> bool {
        self.kind == TurnFailureKind::ContextLimit
    }

    pub(crate) fn allows_fallback(&self) -> bool {
        !self.unsafe_output
            && matches!(
                self.kind,
                TurnFailureKind::ContextLimit | TurnFailureKind::RetryableProfile
            )
    }

    pub(super) fn cancelled() -> Self {
        Self::new(TurnFailureKind::Cancelled, "cancelled")
    }

    pub(crate) fn terminal(message: impl Into<String>) -> Self {
        Self::new(TurnFailureKind::Terminal, message)
    }

    pub(super) fn new(kind: TurnFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            unsafe_output: false,
        }
    }

    pub(super) fn after_output(mut self, unsafe_output: bool) -> Self {
        self.unsafe_output |= unsafe_output;
        self
    }
}

impl std::fmt::Display for TurnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Runs model turns until the session produces a final answer or reaches its independent limit.
pub(crate) async fn run_turns(core: &CoreState, state: &AgentTurnState) -> Result<(), TurnFailure> {
    let mut remaining = MAX_MODEL_TURNS;
    run_turns_with_limit(core, state, &mut remaining).await
}

pub(crate) async fn run_turns_with_limit(
    core: &CoreState,
    state: &AgentTurnState,
    remaining: &mut usize,
) -> Result<(), TurnFailure> {
    let mut active_image_bytes = history_image_bytes(state);
    let mut ephemeral_entries = 0;
    while *remaining > 0 {
        *remaining -= 1;
        if let Err(error) = stream::cancel_if_requested(core, state).await {
            remove_ephemeral_history(state, &mut ephemeral_entries);
            return Err(error);
        }
        let turn = stream::run_model_turn(core, state).await;
        remove_ephemeral_history(state, &mut ephemeral_entries);
        let turn = turn?;
        stream::cancel_if_requested(core, state).await?;
        let assistant_entry = HistoryEntry {
            message: Message::new(MessageRole::Assistant, turn.text),
            attachments: Vec::new(),
            tool_calls: turn.tool_calls.clone(),
            tool_results: Vec::new(),
            provider_metadata: turn.metadata,
        };
        if turn.tool_calls.is_empty() {
            push_history(core, state, assistant_entry, true);
            return Ok(());
        }
        let prepared = super::preflight::prepare_tool_batch(core, state, &turn.tool_calls);
        let ephemeral = prepared.halted;
        push_history(core, state, assistant_entry, !ephemeral);
        let results = dispatch_tools(
            core,
            state,
            &turn.tool_calls,
            prepared,
            &mut active_image_bytes,
        )
        .await;
        let tool_entry = HistoryEntry {
            message: Message::new(MessageRole::Tool, ""),
            attachments: Vec::new(),
            tool_calls: Vec::new(),
            tool_results: results,
            provider_metadata: Value::Null,
        };
        push_history(core, state, tool_entry, !ephemeral);
        ephemeral_entries = usize::from(ephemeral).saturating_mul(2);
    }
    remove_ephemeral_history(state, &mut ephemeral_entries);
    Err(TurnFailure::terminal(format!(
        "agent stopped after {MAX_MODEL_TURNS} model turns"
    )))
}

fn remove_ephemeral_history(state: &AgentTurnState, count: &mut usize) {
    if *count == 0 {
        return;
    }
    let mut history = state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned");
    let retained = history.len().saturating_sub(*count);
    history.truncate(retained);
    *count = 0;
}

async fn dispatch_tools(
    core: &CoreState,
    state: &AgentTurnState,
    calls: &[ToolCall],
    prepared: super::preflight::PreparedBatch,
    active_image_bytes: &mut usize,
) -> Vec<crate::ToolResult> {
    let super::preflight::PreparedBatch {
        calls: prepared_calls,
        results: prepared_results,
        halted,
    } = prepared;
    if halted {
        return prepared_results;
    }
    let mut results = Vec::with_capacity(calls.len());
    for (original, prepared) in calls.iter().zip(prepared_calls) {
        let Some(call) = prepared else {
            let result = super::preflight::prepared_result(&prepared_results, original);
            emit_tool_result(core, state, result.clone());
            results.push(result);
            continue;
        };
        let result = if state.active().cancelled.load(Ordering::Acquire) {
            crate::ToolResult::error(&call.id, "tool dispatch cancelled")
        } else if state
            .allowed_tools()
            .is_some_and(|tools| !tools.contains(&call.name))
        {
            crate::ToolResult::error(
                &call.id,
                format!("tool `{}` is not allowed for this agent role", call.name),
            )
        } else if call.name == "view_image"
            && !core.model_supports(state.model(), InputModality::Image)
        {
            crate::ToolResult::error(
                &call.id,
                "view_image is unavailable because the selected model lacks image input",
            )
        } else if super::super::tool_router::is_core_tool(&call.name) {
            super::super::tool_router::dispatch(core, state, &call).await
        } else {
            match state.identity() {
                super::AgentTurnIdentity::Main => {
                    core.dispatcher
                        .dispatch_cancellable(&call, state.active().cancellation_receiver())
                        .await
                }
                super::AgentTurnIdentity::Child(id) => {
                    core.dispatcher
                        .dispatch_for_owner(
                            &call,
                            ActivityOwner::Agent(id),
                            state.active().cancellation_receiver(),
                        )
                        .await
                }
            }
        };
        let image_bytes = result.attachments.iter().fold(0_usize, |total, image| {
            total.saturating_add(image.bytes().len())
        });
        let next = active_image_bytes.saturating_add(image_bytes);
        let result = if next > crate::MAX_ACTIVE_IMAGE_BYTES {
            crate::ToolResult::error(
                &call.id,
                "tool image exceeds the active provider-request image budget",
            )
        } else {
            *active_image_bytes = next;
            result
        };
        emit_tool_result(core, state, result.clone());
        results.push(result);
    }
    results
}

pub(super) fn emit_tool_result(
    core: &CoreState,
    state: &AgentTurnState,
    result: crate::ToolResult,
) {
    if let TurnEventSink::Submission(submission) = state.events() {
        core.emit(&CoreEvent::ToolResult { submission, result });
    }
}

fn push_history(core: &CoreState, state: &AgentTurnState, entry: HistoryEntry, persist: bool) {
    state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned")
        .push(entry.clone());
    if persist && state.persistence() == PersistencePolicy::Conversation {
        core.history
            .lock()
            .expect("canonical history mutex must not be poisoned")
            .push(entry.clone());
        core.persist_history(entry);
    }
}

fn history_image_bytes(state: &AgentTurnState) -> usize {
    state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned")
        .iter()
        .fold(0_usize, |total, entry| {
            let message = entry.attachments.iter().fold(0_usize, |sum, image| {
                sum.saturating_add(image.bytes().len())
            });
            entry
                .tool_results
                .iter()
                .fold(total.saturating_add(message), |sum, result| {
                    result.attachments.iter().fold(sum, |bytes, image| {
                        bytes.saturating_add(image.bytes().len())
                    })
                })
        })
}

pub(super) fn serialized_history(state: &AgentTurnState, include_images: bool) -> Vec<Value> {
    state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned")
        .iter()
        .map(|entry| serialize_history_entry(entry, include_images))
        .collect()
}

pub(super) fn tool_definitions(
    core: &CoreState,
    state: &AgentTurnState,
    supports_images: bool,
) -> Vec<crate::ToolDefinition> {
    let mut definitions = core.dispatcher.definitions_for_agent(
        supports_images,
        core.client_capabilities.question_request == Some(1),
        state.allowed_tools(),
    );
    if let Some(spawn) = definitions
        .iter_mut()
        .find(|definition| definition.name == "spawn_agent")
    {
        let catalog = state.role_catalog();
        if !catalog.is_empty() {
            spawn.description.push_str(" Effective roles: ");
            spawn.description.push_str(catalog);
        }
    }
    definitions
}

pub(in crate::core) fn serialize_history_entry(
    entry: &HistoryEntry,
    include_images: bool,
) -> Value {
    let role = match entry.message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    let mut value = json!({
        "role": role,
        "content": entry.message.content,
        "tool_calls": entry.tool_calls,
        "tool_results": entry.tool_results,
        "provider_metadata": entry.provider_metadata,
    });
    if !include_images {
        let results = value.get_mut("tool_results").and_then(Value::as_array_mut);
        if let Some(results) = results {
            for result in results {
                result["attachments"] = Value::Array(Vec::new());
            }
        }
    }
    if include_images && !entry.attachments.is_empty() {
        value["attachments"] = json!(entry.attachments);
    }
    value
}
