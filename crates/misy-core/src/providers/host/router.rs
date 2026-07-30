//! JSON-RPC response routing and provider notification fan-out.

use super::PendingFailure;
use super::ProviderEvent;
use crate::ProviderId;
use crate::fanout::Fanout;
use crate::providers::redaction::sanitize_remote_message;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

pub(super) type TransportStateLock = Arc<Mutex<TransportState>>;
type PendingSender = oneshot::Sender<Result<Value, PendingFailure>>;

/// Short-lived response routing state; it is never held across async I/O.
#[derive(Debug, Default)]
pub(super) struct TransportState {
    pub(super) pending: BTreeMap<u64, PendingSender>,
    pub(super) streams: BTreeMap<u64, UnboundedSender<Result<ProviderEvent, PendingFailure>>>,
    pub(super) failure: Option<PendingFailure>,
}

pub(super) fn route_message(
    provider: &ProviderId,
    message: &Value,
    state: &TransportStateLock,
    subscribers: &Arc<Fanout<ProviderEvent>>,
) -> Result<(), PendingFailure> {
    let object = message
        .as_object()
        .ok_or_else(|| PendingFailure::Protocol("message must be an object".to_owned()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(PendingFailure::Protocol(
            "jsonrpc must equal `2.0`".to_owned(),
        ));
    }
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if object.contains_key("id") {
            return Err(PendingFailure::Protocol(
                "provider requests are not supported".to_owned(),
            ));
        }
        let params = redact_stream_event_params(method, object.get("params"));
        let request_id = params
            .get("request_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PendingFailure::Protocol("stream request_id must be an unsigned integer".to_owned())
            })?;
        let event = ProviderEvent {
            provider: provider.clone(),
            method: method.to_owned(),
            params,
        };
        let route = {
            let mut transport = state
                .lock()
                .expect("provider transport mutex must not be poisoned");
            let route = transport.streams.get(&request_id).cloned();
            if matches!(method, "completed" | "failed") {
                transport.streams.remove(&request_id);
            }
            route
        };
        if let Some(route) = route {
            // A closed receiver belongs to a cancelled collector; its late event is harmless.
            if matches!(method, "text_delta" | "tool_call" | "completed" | "failed") {
                let _ = route.send(Ok(event.clone()));
                subscribers.emit(&event);
            } else {
                let _ = route.send(Err(PendingFailure::Protocol(format!(
                    "unknown provider stream event `{method}`"
                ))));
            }
        } else {
            // Diagnostic subscribers may observe late events, but no active stream can receive
            // them because correlation found no route.
            subscribers.emit(&event);
        }
        return Ok(());
    }
    let id = object.get("id").and_then(Value::as_u64).ok_or_else(|| {
        PendingFailure::Protocol("response id must be an unsigned integer".to_owned())
    })?;
    let response = match (object.get("result"), object.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => Err(parse_remote_error(error)?),
        _ => {
            return Err(PendingFailure::Protocol(
                "response must contain exactly one of result or error".to_owned(),
            ));
        }
    };
    if let Some(sender) = state
        .lock()
        .expect("provider transport mutex must not be poisoned")
        .pending
        .remove(&id)
    {
        // A dropped receiver means the waiter was cancelled or timed out; the response is moot.
        let _ = sender.send(response);
    }
    Ok(())
}

pub(super) fn fail_pending(state: &TransportStateLock, failure: &PendingFailure) -> bool {
    let pending = {
        let mut state = state
            .lock()
            .expect("provider transport mutex must not be poisoned");
        if state.failure.is_some() {
            return false;
        }
        state.failure = Some(failure.clone());
        let pending = std::mem::take(&mut state.pending);
        let streams = std::mem::take(&mut state.streams);
        (pending, streams)
    };
    for sender in pending.0.into_values() {
        // A dropped receiver means the waiter already gave up; the failure stays in state.
        let _ = sender.send(Err(failure.clone()));
    }
    for sender in pending.1.into_values() {
        let _ = sender.send(Err(failure.clone()));
    }
    true
}

fn parse_remote_error(value: &Value) -> Result<PendingFailure, PendingFailure> {
    let object = value
        .as_object()
        .ok_or_else(|| PendingFailure::Protocol("error must be an object".to_owned()))?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| PendingFailure::Protocol("error code must be an integer".to_owned()))?;
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| PendingFailure::Protocol("error message must be a string".to_owned()))?;
    Ok(PendingFailure::Remote {
        code,
        // Provider text reaches clients verbatim; cut secrets and oversize payloads at the
        // wire boundary so no core consumer can leak them (events, history, snapshots).
        message: sanitize_remote_message(message),
        data: object.get("data").cloned(),
    })
}

/// The `failed` stream notification carries a free-form provider message that reaches the
/// client's transcript verbatim; redact it at the wire boundary like remote error responses.
fn redact_stream_event_params(method: &str, params: Option<&Value>) -> Value {
    let mut params = params.cloned().unwrap_or(Value::Null);
    if method == "failed"
        && let Some(Value::String(message)) = params.get_mut("message")
    {
        *message = sanitize_remote_message(message);
    }
    params
}
