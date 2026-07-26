use super::{ActiveSubmission, CoreEvent, HistoryEntry, MisyCore, SubmissionId};
use crate::{Message, MessageRole, ModelRef, ProviderEvent, ProviderId, ToolCall, ToolRegistry};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::Ordering,
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

const MAX_MODEL_TURNS: usize = 64;

struct ModelTurn {
    text: String,
    tool_calls: Vec<ToolCall>,
    metadata: Value,
}

impl MisyCore {
    pub(super) fn run_submission(
        &self,
        id: SubmissionId,
        model: ModelRef,
        message: Message,
        active: Arc<ActiveSubmission>,
    ) {
        let _session = self
            .inner
            .session_operation
            .lock()
            .expect("session operation mutex must not be poisoned");
        if active.cancelled.load(Ordering::Acquire) {
            self.inner
                .active
                .lock()
                .expect("active submissions mutex must not be poisoned")
                .remove(&id.get());
            self.emit(CoreEvent::Cancelled { submission: id });
            return;
        }
        self.emit(CoreEvent::SubmissionStarted {
            submission: id,
            model: model.clone(),
        });
        self.push_history(HistoryEntry {
            message,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            provider_metadata: Value::Null,
        });
        let outcome = (|| -> Result<(), String> {
            for _ in 0..MAX_MODEL_TURNS {
                if active.cancelled.load(Ordering::Acquire) {
                    return Err("cancelled".to_owned());
                }
                let turn = self.run_model_turn(id, &model, &active)?;
                if active.cancelled.load(Ordering::Acquire) {
                    return Err("cancelled".to_owned());
                }
                self.push_history(HistoryEntry {
                    message: Message::new(MessageRole::Assistant, turn.text),
                    tool_calls: turn.tool_calls.clone(),
                    tool_results: Vec::new(),
                    provider_metadata: turn.metadata,
                });
                if turn.tool_calls.is_empty() {
                    return Ok(());
                }
                let results = turn
                    .tool_calls
                    .iter()
                    .map(|call| {
                        if active.cancelled.load(Ordering::Acquire) {
                            return crate::ToolResult::error(&call.id, "tool dispatch cancelled");
                        }
                        let result = self.inner.dispatcher.dispatch(call);
                        self.emit(CoreEvent::ToolResult {
                            submission: id,
                            result: result.clone(),
                        });
                        result
                    })
                    .collect();
                self.push_history(HistoryEntry {
                    message: Message::new(MessageRole::Tool, ""),
                    tool_calls: Vec::new(),
                    tool_results: results,
                    provider_metadata: Value::Null,
                });
            }
            Err(format!("agent stopped after {MAX_MODEL_TURNS} model turns"))
        })();
        self.inner
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .remove(&id.get());
        match outcome {
            Ok(()) => self.emit(CoreEvent::Completed { submission: id }),
            Err(message) if message == "cancelled" || active.cancelled.load(Ordering::Acquire) => {
                self.emit(CoreEvent::Cancelled { submission: id })
            }
            Err(message) => self.emit(CoreEvent::Failed {
                submission: id,
                message,
            }),
        }
    }

    fn run_model_turn(
        &self,
        submission: SubmissionId,
        model: &ModelRef,
        active: &ActiveSubmission,
    ) -> Result<ModelTurn, String> {
        let provider_gate = self.provider_gate(&model.provider);
        let _guard = provider_gate
            .lock()
            .expect("provider gate mutex must not be poisoned");
        let (sender, receiver) = mpsc::channel();
        self.inner
            .routes
            .lock()
            .expect("provider routes mutex must not be poisoned")
            .insert(model.provider.as_str().to_owned(), sender);
        let result = self.start_and_collect_turn(submission, model, active, &receiver);
        self.inner
            .routes
            .lock()
            .expect("provider routes mutex must not be poisoned")
            .remove(model.provider.as_str());
        result
    }

    fn start_and_collect_turn(
        &self,
        submission: SubmissionId,
        model: &ModelRef,
        active: &ActiveSubmission,
        events: &Receiver<ProviderEvent>,
    ) -> Result<ModelTurn, String> {
        let pending = self.inner.host.request_async(&model.provider, "chat.start", json!({
            "provider_id": model.provider.as_str(), "model_id": model.model.as_str(),
            "messages": self.serialized_history(), "tools": ToolRegistry::new().definitions(),
        })).map_err(|error| error.to_string())?;
        *active
            .request
            .lock()
            .expect("active request mutex must not be poisoned") =
            Some((model.provider.clone(), pending.id()));
        let request_id = pending.id().get();
        let (reply_sender, reply_receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = reply_sender.send(pending.wait());
        });
        let (mut response_received, mut completed) = (false, false);
        let (mut text, mut tool_calls, mut stream_metadata, mut response_metadata) =
            (String::new(), Vec::new(), Vec::new(), Value::Null);
        loop {
            if active.cancelled.load(Ordering::Acquire) {
                if let Some((provider, request)) = active
                    .request
                    .lock()
                    .expect("active request mutex must not be poisoned")
                    .clone()
                {
                    let _ = self.inner.host.cancel_request(&provider, request);
                }
                return Err("cancelled".to_owned());
            }
            if let Ok(result) = reply_receiver.try_recv() {
                let value = result.map_err(|error| error.to_string())?;
                if let Some(metadata) = value.get("metadata") {
                    response_metadata = metadata.clone();
                }
                response_received = true;
                if completed {
                    break;
                }
            }
            match events.recv_timeout(Duration::from_millis(20)) {
                Ok(event) => {
                    let Some(event_request_id) =
                        event.params.get("request_id").and_then(Value::as_u64)
                    else {
                        return Err(
                            "provider stream event is missing numeric request_id".to_owned()
                        );
                    };
                    if event_request_id != request_id {
                        continue;
                    }
                    match event.method.as_str() {
                        "text_delta" => {
                            let delta = event
                                .params
                                .get("delta")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    "provider text_delta is missing string delta".to_owned()
                                })?
                                .to_owned();
                            let metadata =
                                event.params.get("metadata").cloned().unwrap_or(Value::Null);
                            if !metadata.is_null() {
                                stream_metadata.push(metadata.clone());
                            }
                            text.push_str(&delta);
                            self.emit(CoreEvent::TextDelta {
                                submission,
                                delta,
                                provider_metadata: metadata,
                            });
                        }
                        "tool_call" => {
                            let metadata =
                                event.params.get("metadata").cloned().unwrap_or(Value::Null);
                            if !metadata.is_null() {
                                stream_metadata.push(metadata.clone());
                            }
                            let call: ToolCall = serde_json::from_value(event.params)
                                .map_err(|error| format!("invalid provider tool_call: {error}"))?;
                            self.emit(CoreEvent::ToolCall {
                                submission,
                                call: call.clone(),
                                provider_metadata: metadata,
                            });
                            tool_calls.push(call);
                        }
                        "completed" => {
                            completed = true;
                            if let Some(metadata) = event.params.get("metadata") {
                                stream_metadata.push(metadata.clone());
                            }
                            if response_received {
                                break;
                            }
                        }
                        "failed" => {
                            return Err(event
                                .params
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("provider stream failed")
                                .to_owned());
                        }
                        _ => {
                            return Err(format!(
                                "unknown provider stream event `{}`",
                                event.method
                            ));
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("provider event router disconnected".to_owned());
                }
            }
        }
        *active
            .request
            .lock()
            .expect("active request mutex must not be poisoned") = None;
        Ok(ModelTurn {
            text,
            tool_calls,
            metadata: json!({ "stream": stream_metadata, "response": response_metadata }),
        })
    }

    fn provider_gate(&self, provider: &ProviderId) -> Arc<Mutex<()>> {
        let mut gates = self
            .inner
            .provider_gates
            .lock()
            .expect("provider gates mutex must not be poisoned");
        Arc::clone(
            gates
                .entry(provider.as_str().to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    fn serialized_history(&self) -> Vec<Value> {
        self.inner.history.lock().expect("history mutex must not be poisoned").iter().map(|entry| json!({
            "role": match entry.message.role { MessageRole::System => "system", MessageRole::User => "user", MessageRole::Assistant => "assistant", MessageRole::Tool => "tool" },
            "content": entry.message.content, "tool_calls": entry.tool_calls,
            "tool_results": entry.tool_results, "provider_metadata": entry.provider_metadata,
        })).collect()
    }

    fn push_history(&self, entry: HistoryEntry) {
        self.inner
            .history
            .lock()
            .expect("history mutex must not be poisoned")
            .push(entry);
    }
}
