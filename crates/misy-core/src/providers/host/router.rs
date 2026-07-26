//! JSON-RPC response routing and provider notification fan-out.

use super::PendingFailure;
use crate::{ProviderEvent, ProviderId};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

pub(super) type TransportStateLock = Arc<Mutex<TransportState>>;
type PendingSender = oneshot::Sender<Result<Value, PendingFailure>>;

/// Short-lived response routing state; it is never held across async I/O.
#[derive(Debug, Default)]
pub(super) struct TransportState {
    pub(super) pending: BTreeMap<u64, PendingSender>,
    pub(super) failure: Option<PendingFailure>,
}

/// Subscriber delivery is bounded and non-blocking. Full queues drop new events.
#[derive(Debug)]
pub(super) enum ProviderSubscriber {
    Lossy(mpsc::Sender<ProviderEvent>),
    Lossless(mpsc::UnboundedSender<ProviderEvent>),
}

pub(super) fn route_message(
    provider: &ProviderId,
    message: &Value,
    state: &TransportStateLock,
    subscribers: &Arc<Mutex<Vec<ProviderSubscriber>>>,
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
        broadcast(
            subscribers,
            &ProviderEvent {
                provider: provider.clone(),
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            },
        );
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
        std::mem::take(&mut state.pending)
    };
    for sender in pending.into_values() {
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
        message: message.to_owned(),
        data: object.get("data").cloned(),
    })
}

fn broadcast(subscribers: &Arc<Mutex<Vec<ProviderSubscriber>>>, event: &ProviderEvent) {
    subscribers
        .lock()
        .expect("provider subscribers mutex must not be poisoned")
        .retain(|subscriber| match subscriber {
            ProviderSubscriber::Lossy(sender) => match sender.try_send(event.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            },
            ProviderSubscriber::Lossless(sender) => sender.send(event.clone()).is_ok(),
        });
}
