//! Streaming provider turns and ordered local-tool iteration for one queued submission.

use super::{ActiveSubmission, CoreEvent, CoreState, HistoryEntry, SubmissionId};
use crate::{
    Message, MessageRole, ModelRef, ProviderError, ProviderId, ToolCall,
    providers::{PendingProviderRequest, ProviderEvent},
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::mpsc;

const MAX_MODEL_TURNS: usize = 64;

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

impl TurnStream {
    fn record_response(&mut self, response: Result<Value, ProviderError>) -> Result<(), String> {
        let mut value = response.map_err(|error| error.to_string())?;
        if let Some(metadata) = value.get("metadata") {
            self.response_metadata = metadata.clone();
        }
        // Providers attach replacement credentials when the turn silently refreshed them.
        // They are lifted out here so they can be persisted without ever entering metadata,
        // history, or events.
        self.rotated_credentials = value.get_mut("credentials").map(Value::take);
        self.response_received = true;
        Ok(())
    }

    fn record_metadata(&mut self, metadata: Option<Value>) {
        if let Some(metadata) = metadata.filter(|value| !value.is_null()) {
            self.stream_metadata.push(metadata);
        }
    }

    fn is_complete(&self) -> bool {
        self.response_received && self.completed
    }

    fn into_model_turn(self) -> ModelTurn {
        ModelTurn {
            text: self.text,
            tool_calls: self.tool_calls,
            metadata: json!({ "stream": self.stream_metadata, "response": self.response_metadata }),
        }
    }
}

impl CoreState {
    pub(super) async fn run_submission(
        &self,
        id: SubmissionId,
        model: &ModelRef,
        message: Message,
        active: &ActiveSubmission,
    ) -> CoreEvent {
        if active.cancelled.load(Ordering::Acquire) {
            return CoreEvent::Cancelled { submission: id };
        }
        self.emit(&CoreEvent::SubmissionStarted {
            submission: id,
            model: model.clone(),
        });
        self.push_history(HistoryEntry {
            message,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            provider_metadata: Value::Null,
        });
        let outcome = self.run_agent_turns(id, model, active).await;
        match outcome {
            Ok(()) => CoreEvent::Completed { submission: id },
            Err(message) if message == "cancelled" || active.cancelled.load(Ordering::Acquire) => {
                CoreEvent::Cancelled { submission: id }
            }
            Err(message) => CoreEvent::Failed {
                submission: id,
                message,
            },
        }
    }

    async fn run_agent_turns(
        &self,
        id: SubmissionId,
        model: &ModelRef,
        active: &ActiveSubmission,
    ) -> Result<(), String> {
        for _ in 0..MAX_MODEL_TURNS {
            self.cancel_if_requested(active).await?;
            let turn = self.run_model_turn(id, model, active).await?;
            self.cancel_if_requested(active).await?;
            self.push_history(HistoryEntry {
                message: Message::new(MessageRole::Assistant, turn.text),
                tool_calls: turn.tool_calls.clone(),
                tool_results: Vec::new(),
                provider_metadata: turn.metadata,
            });
            if turn.tool_calls.is_empty() {
                return Ok(());
            }
            let mut results = Vec::with_capacity(turn.tool_calls.len());
            for call in &turn.tool_calls {
                let result = if active.cancelled.load(Ordering::Acquire) {
                    crate::ToolResult::error(&call.id, "tool dispatch cancelled")
                } else {
                    self.dispatcher.dispatch(call).await
                };
                self.emit(&CoreEvent::ToolResult {
                    submission: id,
                    result: result.clone(),
                });
                results.push(result);
            }
            self.push_history(HistoryEntry {
                message: Message::new(MessageRole::Tool, ""),
                tool_calls: Vec::new(),
                tool_results: results,
                provider_metadata: Value::Null,
            });
        }
        Err(format!("agent stopped after {MAX_MODEL_TURNS} model turns"))
    }

    async fn run_model_turn(
        &self,
        submission: SubmissionId,
        model: &ModelRef,
        active: &ActiveSubmission,
    ) -> Result<ModelTurn, String> {
        let provider_gate = self.provider_gate(&model.provider);
        let _guard = provider_gate.lock().await;
        self.refresh_expiring_credentials(&model.provider)
            .await
            .map_err(|error| error.to_string())?;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        self.routes
            .lock()
            .expect("provider routes mutex must not be poisoned")
            .insert(model.provider.clone(), sender);
        let result = self
            .start_and_collect_turn(submission, model, active, &mut receiver)
            .await;
        self.routes
            .lock()
            .expect("provider routes mutex must not be poisoned")
            .remove(&model.provider);
        result
    }

    async fn start_and_collect_turn(
        &self,
        submission: SubmissionId,
        model: &ModelRef,
        active: &ActiveSubmission,
        events: &mut mpsc::UnboundedReceiver<ProviderEvent>,
    ) -> Result<ModelTurn, String> {
        let params = self.chat_params(model).await?;
        let pending = self
            .host
            .request_async(&model.provider, "chat.start", params)
            .await
            .map_err(|error| error.to_string())?;
        let request_id = pending.id().get();
        *active
            .request
            .lock()
            .expect("active request mutex must not be poisoned") =
            Some((model.provider.clone(), pending.id()));
        let stream = self
            .collect_stream(
                submission,
                &model.provider,
                request_id,
                active,
                events,
                pending,
            )
            .await;
        *active
            .request
            .lock()
            .expect("active request mutex must not be poisoned") = None;
        let mut stream = stream?;
        if let Some(credentials) = stream.rotated_credentials.take() {
            // Reuse the auth persistence path so a silent rotation updates stored credentials,
            // authentication state, and the model cache exactly like an explicit refresh.
            let mut response = json!({ "credentials": credentials });
            self.store_credentials(&model.provider, &mut response, true)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(stream.into_model_turn())
    }

    async fn chat_params(&self, model: &ModelRef) -> Result<Value, String> {
        let mut params = json!({
            "provider_id": model.provider.as_str(),
            "model_id": model.model.as_str(),
            "messages": self.serialized_history(),
            "tools": self.dispatcher.definitions(),
        });
        if let Some(credentials) = self
            .load_credentials(&model.provider)
            .await
            .map_err(|error| error.to_string())?
        {
            params["credentials"] = credentials;
        }
        Ok(params)
    }

    async fn collect_stream(
        &self,
        submission: SubmissionId,
        provider: &ProviderId,
        request_id: u64,
        active: &ActiveSubmission,
        events: &mut mpsc::UnboundedReceiver<ProviderEvent>,
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
        let mut cancellation = active.cancellation_receiver();
        let idle = self.host.deadlines().stream_idle;
        loop {
            if active.cancelled.load(Ordering::Acquire) {
                self.cancel_if_requested(active).await?;
            }
            // Legitimate answers stream for minutes, so the turn gets no overall deadline; only
            // silence between events is bounded, because a live provider that stops emitting is
            // hung and would otherwise hold the per-provider gate forever.
            // rustfmt leaves `select!` bodies alone, so the response branch is a named future
            // to stay within the column limit. It borrows `response` instead of taking the wait
            // out, because the branch is dropped mid-poll whenever an event wins the race, and
            // the pending wait must survive that to be polled again on the next iteration.
            // Clearing after the `select!` keeps the borrow and the handler from conflicting.
            let response_pending = response.is_some();
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
                        stream.record_response(result)?;
                        response_done = true;
                    }
                    event = events.recv() => match event {
                        Some(event) => {
                            self.consume_stream_event(event, request_id, submission, &mut stream)?;
                        }
                        None => return Err("provider event router disconnected".to_owned()),
                    },
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            self.cancel_if_requested(active).await?;
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
                Ok(Err(message)) => return Err(message),
                Err(_) => {
                    let message = format!(
                        "provider `{}` produced no stream activity for {}s",
                        provider.as_str(),
                        idle.as_secs()
                    );
                    // The subprocess is hung but alive: fail it so the next request spawns a
                    // fresh provider instead of queueing behind the stuck one.
                    self.host.fail_provider(provider, message.clone()).await;
                    return Err(message);
                }
            }
            if stream.is_complete() {
                return Ok(stream);
            }
        }
    }

    async fn cancel_if_requested(&self, active: &ActiveSubmission) -> Result<(), String> {
        if !active.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let request = active
            .request
            .lock()
            .expect("active request mutex must not be poisoned")
            .clone();
        if let Some((provider, request)) = request {
            // Local cancellation is authoritative; remote cancellation only accelerates cleanup.
            let _ = self.host.cancel_request(&provider, request).await;
        }
        Err("cancelled".to_owned())
    }

    fn consume_stream_event(
        &self,
        event: ProviderEvent,
        request_id: u64,
        submission: SubmissionId,
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
            "text_delta" => self.append_text(&event.params, submission, stream),
            "tool_call" => self.append_tool_call(event.params, submission, stream),
            "completed" => {
                stream.completed = true;
                stream.record_metadata(event.params.get("metadata").cloned());
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
        &self,
        params: &Value,
        submission: SubmissionId,
        stream: &mut TurnStream,
    ) -> Result<(), String> {
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .ok_or_else(|| "provider text_delta is missing string delta".to_owned())?
            .to_owned();
        let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
        stream.record_metadata(Some(metadata.clone()));
        stream.text.push_str(&delta);
        self.emit(&CoreEvent::TextDelta {
            submission,
            delta,
            provider_metadata: metadata,
        });
        Ok(())
    }

    fn append_tool_call(
        &self,
        params: Value,
        submission: SubmissionId,
        stream: &mut TurnStream,
    ) -> Result<(), String> {
        let metadata = params.get("metadata").cloned().unwrap_or(Value::Null);
        stream.record_metadata(Some(metadata.clone()));
        let call: ToolCall = serde_json::from_value(params)
            .map_err(|error| format!("invalid provider tool_call: {error}"))?;
        self.emit(&CoreEvent::ToolCall {
            submission,
            call: call.clone(),
            provider_metadata: metadata,
        });
        stream.tool_calls.push(call);
        Ok(())
    }

    fn provider_gate(&self, provider: &ProviderId) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self
            .provider_gates
            .lock()
            .expect("provider gates mutex must not be poisoned");
        Arc::clone(
            gates
                .entry(provider.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    fn serialized_history(&self) -> Vec<Value> {
        self.history
            .lock()
            .expect("history mutex must not be poisoned")
            .iter()
            .map(serialize_history_entry)
            .collect()
    }

    fn push_history(&self, entry: HistoryEntry) {
        self.history
            .lock()
            .expect("history mutex must not be poisoned")
            .push(entry);
    }

    pub(super) fn emit(&self, event: &CoreEvent) {
        self.subscribers.emit(event);
    }
}

fn serialize_history_entry(entry: &HistoryEntry) -> Value {
    let role = match entry.message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    json!({
        "role": role,
        "content": entry.message.content,
        "tool_calls": entry.tool_calls,
        "tool_results": entry.tool_results,
        "provider_metadata": entry.provider_metadata,
    })
}
