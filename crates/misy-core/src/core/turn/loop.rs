//! Shared provider streaming and local-tool loop for one [`AgentTurnState`](super::AgentTurnState).

use super::super::CoreState;
use super::{AgentTurnState, PersistencePolicy, TurnEventSink};
use crate::{
    CoreEvent, HistoryEntry, InputModality, Message, MessageRole, ProviderError, ProviderId,
    ProviderRequestId, ToolCall,
    activity::ActivityOwner,
    providers::{
        MAX_PROTOCOL_FRAME_BYTES, PendingProviderRequest, ProviderEvent, ProviderStreamReceiver,
    },
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(crate) const MAX_MODEL_TURNS: usize = 64;

struct ModelTurn {
    text: String,
    tool_calls: Vec<ToolCall>,
    metadata: Value,
}

struct TurnStream {
    text: String,
    tool_calls: Vec<ToolCall>,
    stream_metadata: Vec<Value>,
    response_metadata: Value,
    rotated_credentials: Option<Value>,
    response_received: bool,
    completed: bool,
}

/// Runs model turns until the session produces a final answer or reaches its independent limit.
pub(crate) async fn run_turns(core: &CoreState, state: &AgentTurnState) -> Result<(), String> {
    let mut remaining = MAX_MODEL_TURNS;
    run_turns_with_limit(core, state, &mut remaining).await
}

pub(crate) async fn run_turns_with_limit(
    core: &CoreState,
    state: &AgentTurnState,
    remaining: &mut usize,
) -> Result<(), String> {
    let mut active_image_bytes = history_image_bytes(state);
    while *remaining > 0 {
        *remaining -= 1;
        cancel_if_requested(core, state).await?;
        let turn = run_model_turn(core, state).await?;
        cancel_if_requested(core, state).await?;
        push_history(
            core,
            state,
            HistoryEntry {
                message: Message::new(MessageRole::Assistant, turn.text),
                attachments: Vec::new(),
                tool_calls: turn.tool_calls.clone(),
                tool_results: Vec::new(),
                provider_metadata: turn.metadata,
            },
        );
        if turn.tool_calls.is_empty() {
            return Ok(());
        }
        let results = dispatch_tools(core, state, &turn.tool_calls, &mut active_image_bytes).await;
        push_history(
            core,
            state,
            HistoryEntry {
                message: Message::new(MessageRole::Tool, ""),
                attachments: Vec::new(),
                tool_calls: Vec::new(),
                tool_results: results,
                provider_metadata: Value::Null,
            },
        );
    }
    Err(format!("agent stopped after {MAX_MODEL_TURNS} model turns"))
}

async fn dispatch_tools(
    core: &CoreState,
    state: &AgentTurnState,
    calls: &[ToolCall],
    active_image_bytes: &mut usize,
) -> Vec<crate::ToolResult> {
    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        let result = if state.active().cancelled.load(Ordering::Acquire) {
            crate::ToolResult::error(&call.id, "tool dispatch cancelled")
        } else if call.name == "view_image"
            && !core.model_supports(state.model(), InputModality::Image)
        {
            crate::ToolResult::error(
                &call.id,
                "view_image is unavailable because the selected model lacks image input",
            )
        } else if super::super::tool_router::is_core_tool(&call.name) {
            super::super::tool_router::dispatch(core, state, call).await
        } else {
            match state.identity() {
                super::AgentTurnIdentity::Main => {
                    core.dispatcher
                        .dispatch_cancellable(call, state.active().cancellation_receiver())
                        .await
                }
                super::AgentTurnIdentity::Child(id) => {
                    core.dispatcher
                        .dispatch_for_owner(
                            call,
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

async fn run_model_turn(core: &CoreState, state: &AgentTurnState) -> Result<ModelTurn, String> {
    core.refresh_expiring_credentials(&state.model().provider)
        .await
        .map_err(|error| error.to_string())?;
    start_and_collect_turn(core, state).await
}

async fn start_and_collect_turn(
    core: &CoreState,
    state: &AgentTurnState,
) -> Result<ModelTurn, String> {
    let model = state.model();
    let credential_epoch = core.credential_epoch(&model.provider);
    let params = chat_params(core, state).await?;
    let chat = core
        .host
        .start_chat(&model.provider, params)
        .await
        .map_err(|error| error.to_string())?;
    let request_id = chat.id().get();
    let (pending, mut events) = chat.into_parts();
    *state
        .active()
        .request
        .lock()
        .expect("turn request mutex must not be poisoned") =
        Some((model.provider.clone(), pending.id()));
    let stream = collect_stream(
        core,
        state,
        &model.provider,
        request_id,
        &mut events,
        pending,
    )
    .await;
    *state
        .active()
        .request
        .lock()
        .expect("turn request mutex must not be poisoned") = None;
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            let _ = core
                .host
                .cancel_request(&model.provider, ProviderRequestId(request_id))
                .await;
            return Err(error);
        }
    };
    if let Some(credentials) = stream.rotated_credentials.take() {
        let mut response = json!({ "credentials": credentials });
        core.store_credentials_if_epoch(&model.provider, credential_epoch, &mut response, true)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(ModelTurn {
        text: stream.text,
        tool_calls: stream.tool_calls,
        metadata: json!({ "stream": stream.stream_metadata, "response": stream.response_metadata }),
    })
}

async fn chat_params(core: &CoreState, state: &AgentTurnState) -> Result<Value, String> {
    let supports_images = core.model_supports(state.model(), InputModality::Image);
    let mut params = json!({
        "provider_id": state.model().provider.as_str(),
        "model_id": state.model().model.as_str(),
        "messages": serialized_history(state, supports_images),
        "tools": tool_definitions(core, state, supports_images),
    });
    if let Some(credentials) = core
        .load_credentials(&state.model().provider)
        .await
        .map_err(|error| error.to_string())?
    {
        params["credentials"] = credentials;
    }
    let envelope =
        json!({ "jsonrpc": "2.0", "id": u64::MAX, "method": "chat.start", "params": &params });
    let encoded_bytes = serde_json::to_vec(&envelope)
        .map_err(|error| format!("could not encode chat request: {error}"))?
        .len()
        .saturating_add(1);
    if encoded_bytes > MAX_PROTOCOL_FRAME_BYTES {
        return Err(format!(
            "chat request contains {encoded_bytes} bytes; provider frame limit is \
             {MAX_PROTOCOL_FRAME_BYTES}"
        ));
    }
    Ok(params)
}

async fn collect_stream(
    core: &CoreState,
    state: &AgentTurnState,
    provider: &ProviderId,
    request_id: u64,
    events: &mut ProviderStreamReceiver,
    pending: PendingProviderRequest,
) -> Result<TurnStream, String> {
    let mut stream = TurnStream {
        text: String::new(),
        tool_calls: Vec::new(),
        stream_metadata: Vec::new(),
        response_metadata: Value::Null,
        rotated_credentials: None,
        response_received: false,
        completed: false,
    };
    let mut response = Some(Box::pin(pending.wait()));
    let mut cancellation = state.active().cancellation_receiver();
    let idle = core.host.deadlines().stream_idle;
    let mut stream_closed = false;
    loop {
        if state.active().cancelled.load(Ordering::Acquire) {
            cancel_if_requested(core, state).await?;
        }
        let response_pending = response.is_some();
        let stream_open = !stream_closed;
        let response_ready = async {
            response
                .as_mut()
                .expect("guarded by response_pending")
                .await
        };
        let mut response_done = false;
        let step = tokio::time::timeout(idle, async {
            tokio::select! {
                result = response_ready, if response_pending => {
                    record_response(&mut stream, result)?;
                    response_done = true;
                }
                event = events.recv(), if stream_open => match event {
                    Some(Ok(event)) => {
                        consume_stream_event(core, state, event, request_id, &mut stream)?;
                    }
                    Some(Err(error)) => {
                        return Err(error
                            .into_error(provider, ProviderRequestId(request_id))
                            .to_string());
                    }
                    None if stream.completed => stream_closed = true,
                    None => return Err("provider event router disconnected".to_owned()),
                },
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        cancel_if_requested(core, state).await?;
                    }
                }
            }
            Ok::<(), String>(())
        })
        .await;
        if response_done {
            response = None;
        }
        match step {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                let message = format!(
                    "provider `{}` produced no stream activity for {}s",
                    provider.as_str(),
                    idle.as_secs()
                );
                core.host.fail_provider(provider, message.clone()).await;
                return Err(message);
            }
        }
        if stream.response_received && stream.completed {
            return Ok(stream);
        }
    }
}

fn record_response(
    stream: &mut TurnStream,
    response: Result<Value, ProviderError>,
) -> Result<(), String> {
    let mut value = response.map_err(|error| error.to_string())?;
    if let Some(metadata) = value.get("metadata") {
        stream.response_metadata = metadata.clone();
    }
    stream.rotated_credentials = value.get_mut("credentials").map(Value::take);
    stream.response_received = true;
    Ok(())
}

async fn cancel_if_requested(core: &CoreState, state: &AgentTurnState) -> Result<(), String> {
    if !state.active().cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let request = state
        .active()
        .request
        .lock()
        .expect("turn request mutex must not be poisoned")
        .clone();
    if let Some((provider, request)) = request {
        let _ = core.host.cancel_request(&provider, request).await;
    }
    Err("cancelled".to_owned())
}

fn consume_stream_event(
    core: &CoreState,
    state: &AgentTurnState,
    event: ProviderEvent,
    request_id: u64,
    stream: &mut TurnStream,
) -> Result<(), String> {
    let event_request_id = event
        .params
        .get("request_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "provider stream event is missing numeric request_id".to_owned())?;
    if event_request_id != request_id {
        return Ok(());
    }
    match event.method.as_str() {
        "text_delta" => append_text(core, state, &event.params, stream),
        "tool_call" => append_tool_call(core, state, event.params, stream),
        "completed" => {
            stream.completed = true;
            record_metadata(stream, event.params.get("metadata").cloned());
            Ok(())
        }
        "failed" => Err(event
            .params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider stream failed")
            .to_owned()),
        _ => Err(format!("unknown provider stream event `{}`", event.method)),
    }
}

fn append_text(
    core: &CoreState,
    state: &AgentTurnState,
    params: &Value,
    stream: &mut TurnStream,
) -> Result<(), String> {
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider text_delta is missing string delta".to_owned())?
        .to_owned();
    let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
    record_metadata(stream, Some(metadata.clone()));
    stream.text.push_str(&delta);
    if let TurnEventSink::Submission(submission) = state.events() {
        core.emit(&CoreEvent::TextDelta {
            submission,
            delta,
            provider_metadata: metadata,
        });
    }
    Ok(())
}

fn append_tool_call(
    core: &CoreState,
    state: &AgentTurnState,
    params: Value,
    stream: &mut TurnStream,
) -> Result<(), String> {
    let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
    record_metadata(stream, Some(metadata.clone()));
    let call: ToolCall = serde_json::from_value(params)
        .map_err(|error| format!("invalid provider tool_call: {error}"))?;
    if let TurnEventSink::Submission(submission) = state.events() {
        core.emit(&CoreEvent::ToolCall {
            submission,
            call: call.clone(),
            provider_metadata: metadata,
        });
    }
    stream.tool_calls.push(call);
    Ok(())
}

fn record_metadata(stream: &mut TurnStream, metadata: Option<Value>) {
    if let Some(metadata) = metadata.filter(|value| !value.is_null()) {
        stream.stream_metadata.push(metadata);
    }
}

fn emit_tool_result(core: &CoreState, state: &AgentTurnState, result: crate::ToolResult) {
    if let TurnEventSink::Submission(submission) = state.events() {
        core.emit(&CoreEvent::ToolResult { submission, result });
    }
}

fn push_history(core: &CoreState, state: &AgentTurnState, entry: HistoryEntry) {
    state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned")
        .push(entry.clone());
    if state.persistence() == PersistencePolicy::Conversation {
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

fn serialized_history(state: &AgentTurnState, include_images: bool) -> Vec<Value> {
    state
        .history()
        .lock()
        .expect("turn history mutex must not be poisoned")
        .iter()
        .map(|entry| serialize_history_entry(entry, include_images))
        .collect()
}

fn tool_definitions(
    core: &CoreState,
    state: &AgentTurnState,
    supports_images: bool,
) -> Vec<crate::ToolDefinition> {
    if state.permits_agent_tools() {
        core.dispatcher.definitions_for_client(
            supports_images,
            core.client_capabilities.question_request == Some(1),
        )
    } else {
        core.dispatcher.definitions_for_child(
            supports_images,
            core.client_capabilities.question_request == Some(1),
        )
    }
}

fn serialize_history_entry(entry: &HistoryEntry, include_images: bool) -> Value {
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
