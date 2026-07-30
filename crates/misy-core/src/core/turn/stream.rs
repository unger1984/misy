//! Provider request composition, stream collection, and typed failure classification.

use super::{
    AgentTurnState, TurnEventSink,
    r#loop::{TurnFailure, TurnFailureKind},
};
use crate::{
    CoreError, CoreEvent, InputModality, ProviderError, ProviderId, ProviderRequestId, ToolCall,
    core::CoreState,
    providers::{
        MAX_PROTOCOL_FRAME_BYTES, PendingProviderRequest, ProviderEvent, ProviderStreamReceiver,
    },
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(super) struct ModelTurn {
    pub(super) text: String,
    pub(super) tool_calls: Vec<ToolCall>,
    pub(super) metadata: Value,
}

struct TurnStream {
    text: String,
    tool_calls: Vec<ToolCall>,
    stream_metadata: Vec<Value>,
    response_metadata: Value,
    rotated_credentials: Option<Value>,
    response_received: bool,
    completed: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

pub(super) async fn run_model_turn(
    core: &CoreState,
    state: &AgentTurnState,
) -> Result<ModelTurn, TurnFailure> {
    core.refresh_expiring_credentials(&state.model().provider)
        .await
        .map_err(classify_core_error)?;
    start_and_collect_turn(core, state).await
}

async fn start_and_collect_turn(
    core: &CoreState,
    state: &AgentTurnState,
) -> Result<ModelTurn, TurnFailure> {
    let model = state.model();
    let credential_epoch = core.credential_epoch(&model.provider);
    let params = chat_params(core, state)
        .await
        .map_err(TurnFailure::terminal)?;
    let chat = core
        .host
        .start_chat(&model.provider, params)
        .await
        .map_err(classify_provider_error)?;
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
            .map_err(classify_core_error)?;
    }
    Ok(ModelTurn {
        text: stream.text,
        tool_calls: stream.tool_calls,
        metadata: json!({
            "stream": stream.stream_metadata,
            "response": stream.response_metadata,
            "input_tokens": stream.input_tokens,
            "output_tokens": stream.output_tokens,
        }),
    })
}

async fn chat_params(core: &CoreState, state: &AgentTurnState) -> Result<Value, String> {
    let supports_images = core.model_supports(state.model(), InputModality::Image);
    let system_prompt = state.system_override().map_or_else(
        || {
            state
                .instructions()
                .lock()
                .expect("instruction session mutex must not be poisoned")
                .rendered_with_role(
                    state.identity() != super::AgentTurnIdentity::Main,
                    state.role_instructions(),
                )
                .system_prompt
        },
        ToOwned::to_owned,
    );
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt,
        "tool_calls": [],
        "tool_results": [],
        "provider_metadata": Value::Null,
    })];
    messages.extend(super::r#loop::serialized_history(state, supports_images));
    let mut params = json!({
        "provider_id": state.model().provider.as_str(),
        "model_id": state.model().model.as_str(),
        "messages": messages,
        "tools": super::r#loop::tool_definitions(core, state, supports_images),
    });
    if let Some(credentials) = core
        .load_credentials(&state.model().provider)
        .await
        .map_err(|error| error.to_string())?
    {
        params["credentials"] = credentials;
    }
    let supports_thinking = core
        .catalog
        .get(&state.model().provider)
        .is_some_and(|package| package.manifest().supports_capability("thinking", 1));
    if supports_thinking && let Some(thinking) = state.thinking() {
        params["thinking"] = Value::String(thinking.to_owned());
    }
    if let Some(max_output_tokens) = state.max_output_tokens() {
        params["max_output_tokens"] = json!(max_output_tokens);
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
) -> Result<TurnStream, TurnFailure> {
    let mut stream = TurnStream {
        text: String::new(),
        tool_calls: Vec::new(),
        stream_metadata: Vec::new(),
        response_metadata: Value::Null,
        rotated_credentials: None,
        response_received: false,
        completed: false,
        input_tokens: None,
        output_tokens: None,
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
                    let unsafe_output = stream_has_unsafe_output(&stream);
                    record_response(&mut stream, result)
                        .map_err(|error| error.after_output(unsafe_output))?;
                    response_done = true;
                }
                event = events.recv(), if stream_open => match event {
                    Some(Ok(event)) => {
                        let unsafe_output = stream_has_unsafe_output(&stream);
                        consume_stream_event(core, state, event, request_id, &mut stream)
                            .map_err(|error| error.after_output(unsafe_output))?;
                    }
                    Some(Err(error)) => {
                        return Err(classify_provider_error(
                            error.into_error(provider, ProviderRequestId(request_id)),
                        )
                        .after_output(stream_has_unsafe_output(&stream)));
                    }
                    None if stream.completed => stream_closed = true,
                    None => {
                        return Err(TurnFailure::new(
                            TurnFailureKind::RetryableProfile,
                            "provider event router disconnected",
                        )
                        .after_output(stream_has_unsafe_output(&stream)));
                    }
                },
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        cancel_if_requested(core, state).await?;
                    }
                }
            }
            Ok::<(), TurnFailure>(())
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
                return Err(TurnFailure::new(TurnFailureKind::RetryableProfile, message)
                    .after_output(stream_has_unsafe_output(&stream)));
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
) -> Result<(), TurnFailure> {
    let mut value = response.map_err(classify_provider_error)?;
    if let Some(metadata) = value.get("metadata") {
        stream.response_metadata = metadata.clone();
    }
    stream.rotated_credentials = value.get_mut("credentials").map(Value::take);
    stream.response_received = true;
    Ok(())
}

pub(super) async fn cancel_if_requested(
    core: &CoreState,
    state: &AgentTurnState,
) -> Result<(), TurnFailure> {
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
    Err(TurnFailure::cancelled())
}

fn consume_stream_event(
    core: &CoreState,
    state: &AgentTurnState,
    event: ProviderEvent,
    request_id: u64,
    stream: &mut TurnStream,
) -> Result<(), TurnFailure> {
    let event_request_id = event
        .params
        .get("request_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            TurnFailure::terminal("provider stream event is missing numeric request_id")
        })?;
    if event_request_id != request_id {
        return Ok(());
    }
    match event.method.as_str() {
        "text_delta" => append_text(core, state, &event.params, stream),
        "tool_call" => append_tool_call(core, state, event.params, stream),
        "completed" => {
            stream.completed = true;
            stream.input_tokens = event.params.get("input_tokens").and_then(Value::as_u64);
            stream.output_tokens = event.params.get("output_tokens").and_then(Value::as_u64);
            record_metadata(stream, event.params.get("metadata").cloned());
            Ok(())
        }
        "failed" => Err(classify_stream_failure(
            event
                .params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider stream failed"),
        )),
        _ => Err(TurnFailure::terminal(format!(
            "unknown provider stream event `{}`",
            event.method
        ))),
    }
}

fn append_text(
    core: &CoreState,
    state: &AgentTurnState,
    params: &Value,
    stream: &mut TurnStream,
) -> Result<(), TurnFailure> {
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .ok_or_else(|| TurnFailure::terminal("provider text_delta is missing string delta"))?
        .to_owned();
    let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
    record_metadata(stream, Some(metadata.clone()));
    stream.text.push_str(&delta);
    if !delta.is_empty() {
        state.close_fallback();
    }
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
) -> Result<(), TurnFailure> {
    let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
    record_metadata(stream, Some(metadata.clone()));
    let call: ToolCall = serde_json::from_value(params)
        .map_err(|error| TurnFailure::terminal(format!("invalid provider tool_call: {error}")))?;
    state.close_fallback();
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

fn stream_has_unsafe_output(stream: &TurnStream) -> bool {
    !stream.text.is_empty() || !stream.tool_calls.is_empty()
}

fn classify_core_error(error: CoreError) -> TurnFailure {
    match error {
        CoreError::Provider(error) => classify_provider_error(error),
        CoreError::ProviderNotAuthenticated(_)
        | CoreError::UnknownModel(_)
        | CoreError::UnsupportedThinking(_)
        | CoreError::UnsupportedInput { .. }
        | CoreError::UnsupportedCapability { .. } => {
            TurnFailure::new(TurnFailureKind::RetryableProfile, error.to_string())
        }
        CoreError::Shutdown => TurnFailure::cancelled(),
        _ => TurnFailure::terminal(error.to_string()),
    }
}

fn classify_provider_error(error: ProviderError) -> TurnFailure {
    let message = error.to_string();
    match error {
        ProviderError::UnknownProvider(_)
        | ProviderError::Spawn { .. }
        | ProviderError::Transport { .. }
        | ProviderError::Timeout { .. } => {
            TurnFailure::new(TurnFailureKind::RetryableProfile, message)
        }
        ProviderError::Remote {
            code,
            message: remote,
            ..
        } => classify_remote_failure(code, &remote, message),
        ProviderError::Cancelled(_) | ProviderError::Shutdown => TurnFailure::cancelled(),
        ProviderError::Protocol { .. } => TurnFailure::terminal(message),
    }
}

fn classify_remote_failure(code: i64, remote: &str, display: String) -> TurnFailure {
    if is_context_limit(remote) {
        return TurnFailure::new(TurnFailureKind::ContextLimit, display);
    }
    if matches!(code, 401 | 403 | 408 | 409 | 429 | 500..=599)
        || is_retryable_profile_message(remote)
    {
        return TurnFailure::new(TurnFailureKind::RetryableProfile, display);
    }
    TurnFailure::terminal(display)
}

fn classify_stream_failure(message: &str) -> TurnFailure {
    if is_context_limit(message) {
        TurnFailure::new(TurnFailureKind::ContextLimit, message)
    } else if is_retryable_profile_message(message) {
        TurnFailure::new(TurnFailureKind::RetryableProfile, message)
    } else {
        TurnFailure::terminal(message)
    }
}

fn is_context_limit(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "context length",
        "context limit",
        "context window",
        "input too long",
        "maximum context",
        "too many input tokens",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_retryable_profile_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "rate limit",
        "quota",
        "overloaded",
        "temporarily unavailable",
        "server unavailable",
        "service unavailable",
        "internal server",
        "network",
        "connection",
        "timed out",
        "timeout",
        "unauthorized",
        "authentication",
        "forbidden",
        "model unavailable",
        "unsupported model",
        "unsupported thinking",
        "unsupported modality",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
