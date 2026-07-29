//! Core-owned lifecycle and public contracts for client-mediated questions.

use super::{AgentId, CoreError, CoreEvent, CoreState, MisyCore, SubmissionId, turn};
use crate::{ToolCall, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{oneshot, watch};

pub(crate) const QUESTION_CAPABILITY_VERSION: u32 = 1;
const MAX_QUESTION_CHARS: usize = 500;
const MAX_OPTION_LABEL_CHARS: usize = 80;
const MAX_OPTION_DESCRIPTION_CHARS: usize = 500;
const MAX_QUESTION_ANSWER_CHARS: usize = 2_000;
const MAX_QUESTION_SERIALIZED_BYTES: usize = 64 * 1024;
const MAX_DISMISSES: u8 = 3;
const DISMISSED_NOTE: &str = "User dismissed the question without answering.";

/// Immutable client capabilities selected when the core is constructed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClientCapabilities {
    /// Supported question-request capability revision, if any.
    pub question_request: Option<u32>,
}

/// Construction options for [`MisyCore`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoreOptions {
    /// Capabilities of the in-process frontend client.
    pub client_capabilities: ClientCapabilities,
}

/// Process-local request identity for an outstanding user question.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct QuestionRequestId(pub(crate) u64);

impl fmt::Display for QuestionRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "question-{}", self.0)
    }
}

/// One selectable answer option.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionOption {
    /// Visible choice label.
    pub label: String,
    /// Optional explanatory text.
    #[serde(default)]
    pub description: String,
}

/// One question in an ordered request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionItem {
    /// Exact text used as the answer-map key.
    pub question: String,
    /// Short tab label.
    #[serde(default)]
    pub header: String,
    /// Ordered selectable choices.
    pub options: Vec<QuestionOption>,
    /// Whether multiple choices may be combined.
    #[serde(default)]
    pub multi_select: bool,
}

/// Owner whose model turn requested a question.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum QuestionSource {
    /// The root conversation submission.
    Submission(SubmissionId),
    /// A process-local child agent.
    Agent(AgentId),
}

/// Client projection of an outstanding request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionRequest {
    /// Stable process-local request identifier.
    pub id: QuestionRequestId,
    /// Provider tool-call identity.
    pub tool_call_id: String,
    /// Turn that owns the request.
    pub source: QuestionSource,
    /// Ordered request contents.
    pub questions: Vec<QuestionItem>,
}

/// A client response delivered to a pending request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionResponse {
    /// Request receiving the response.
    pub request_id: QuestionRequestId,
    /// Exact question-text keys and nonempty answers.
    pub answers: BTreeMap<String, String>,
}

pub(crate) enum QuestionResolution {
    Answered(BTreeMap<String, String>),
    Dismissed,
    Cancelled,
}

struct PendingQuestion {
    request: QuestionRequest,
    response: oneshot::Sender<QuestionResolution>,
}

#[derive(Default)]
struct QuestionRegistryState {
    pending: BTreeMap<QuestionRequestId, PendingQuestion>,
    dismisses: BTreeMap<QuestionSource, u8>,
}

/// Linearizes question registration, client responses, and owner cancellation.
pub(crate) struct QuestionRegistry {
    next_id: AtomicU64,
    state: Mutex<QuestionRegistryState>,
}

impl QuestionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            state: Mutex::new(QuestionRegistryState::default()),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<QuestionRequest> {
        self.state
            .lock()
            .expect("question registry mutex must not be poisoned")
            .pending
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }

    fn register(
        &self,
        source: QuestionSource,
        tool_call_id: String,
        questions: Vec<QuestionItem>,
    ) -> Result<(QuestionRequest, oneshot::Receiver<QuestionResolution>), String> {
        let mut state = self
            .state
            .lock()
            .expect("question registry mutex must not be poisoned");
        if state.dismisses.get(&source).copied().unwrap_or_default() >= MAX_DISMISSES {
            return Err(
                "AskUserQuestion was dismissed three times; continue without asking again"
                    .to_owned(),
            );
        }
        let id = QuestionRequestId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = QuestionRequest {
            id,
            tool_call_id,
            source,
            questions,
        };
        let encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_QUESTION_SERIALIZED_BYTES {
            return Err("question request exceeds 65536 bytes".to_owned());
        }
        let (response, receiver) = oneshot::channel();
        state.pending.insert(
            id,
            PendingQuestion {
                request: request.clone(),
                response,
            },
        );
        Ok((request, receiver))
    }

    fn answer(
        &self,
        response: &QuestionResponse,
    ) -> Result<oneshot::Sender<QuestionResolution>, CoreError> {
        let mut state = self
            .state
            .lock()
            .expect("question registry mutex must not be poisoned");
        let pending = state
            .pending
            .get(&response.request_id)
            .ok_or(CoreError::UnknownQuestion(response.request_id))?;
        validate_answers(&pending.request, response).map_err(CoreError::InvalidQuestionResponse)?;
        let pending = state
            .pending
            .remove(&response.request_id)
            .expect("pending question was checked under the same lock");
        state.dismisses.remove(&pending.request.source);
        Ok(pending.response)
    }

    fn dismiss(
        &self,
        id: QuestionRequestId,
    ) -> Result<oneshot::Sender<QuestionResolution>, CoreError> {
        let mut state = self
            .state
            .lock()
            .expect("question registry mutex must not be poisoned");
        let pending = state
            .pending
            .remove(&id)
            .ok_or(CoreError::UnknownQuestion(id))?;
        let dismisses = state.dismisses.entry(pending.request.source).or_default();
        *dismisses = dismisses.saturating_add(1);
        Ok(pending.response)
    }

    fn take_cancel(&self, id: QuestionRequestId) -> Option<oneshot::Sender<QuestionResolution>> {
        self.state
            .lock()
            .expect("question registry mutex must not be poisoned")
            .pending
            .remove(&id)
            .map(|pending| pending.response)
    }

    pub(crate) fn take_all(&self) -> Vec<(QuestionRequestId, oneshot::Sender<QuestionResolution>)> {
        let pending = std::mem::take(
            &mut self
                .state
                .lock()
                .expect("question registry mutex must not be poisoned")
                .pending,
        );
        pending
            .into_iter()
            .map(|(id, pending)| (id, pending.response))
            .collect()
    }
}

impl MisyCore {
    /// Delivers a complete validated answer to one pending question request.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale request, malformed answer set, or a shut-down core.
    pub fn answer_question(&self, response: QuestionResponse) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let id = response.request_id;
        let sender = self.inner.state.questions.answer(&response)?;
        self.emit(&CoreEvent::QuestionResolved { request_id: id });
        let _ = sender.send(QuestionResolution::Answered(response.answers));
        Ok(())
    }

    /// Dismisses one pending request without cancelling its owning model turn.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is stale or the core has shut down.
    pub fn dismiss_question(&self, request_id: QuestionRequestId) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let sender = self.inner.state.questions.dismiss(request_id)?;
        self.emit(&CoreEvent::QuestionResolved { request_id });
        let _ = sender.send(QuestionResolution::Dismissed);
        Ok(())
    }
}

/// Provider-visible definition for Kimi-compatible structured questions.
pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition::new(
        "AskUserQuestion",
        "Ask the user one to four structured questions and wait for an answer.",
        json!({
            "type": "object", "required": ["questions"],
            "properties": {"questions": {"type": "array", "minItems": 1, "maxItems": 4,
                "items": {"type": "object", "required": ["question", "options"],
                    "properties": {
                        "question": {"type": "string"}, "header": {"type": "string"},
                        "multi_select": {"type": "boolean"},
                        "options": {"type": "array", "minItems": 2, "maxItems": 4,
                            "items": {"type": "object", "required": ["label"],
                                "properties": {"label": {"type": "string"},
                                    "description": {"type": "string"}},
                                "additionalProperties": false}}
                    }, "additionalProperties": false}}
            }, "additionalProperties": false
        }),
    )
}

/// Parses and semantically validates a schema-valid provider payload.
pub(crate) fn parse_questions(value: &Value) -> Result<Vec<QuestionItem>, String> {
    let questions_value = value
        .get("questions")
        .ok_or_else(|| "questions is required".to_owned())?;
    let questions: Vec<QuestionItem> =
        serde_json::from_value(questions_value.clone()).map_err(|error| error.to_string())?;
    let mut seen = BTreeSet::new();
    for question in &questions {
        validate_question(question, &mut seen)?;
    }
    if serde_json::to_vec(value)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_QUESTION_SERIALIZED_BYTES
    {
        return Err("question payload exceeds 65536 bytes".to_owned());
    }
    Ok(questions)
}

fn validate_question(question: &QuestionItem, seen: &mut BTreeSet<String>) -> Result<(), String> {
    let text = question.question.trim();
    if text.is_empty() || text.chars().count() > MAX_QUESTION_CHARS {
        return Err("question must be non-empty and at most 500 characters".to_owned());
    }
    if !seen.insert(question.question.clone()) {
        return Err("question texts must be unique".to_owned());
    }
    if question.header.chars().count() > 12 {
        return Err("question header must be at most 12 characters".to_owned());
    }
    let mut labels = BTreeSet::new();
    for option in &question.options {
        let normalized = option.label.trim();
        if normalized.is_empty()
            || option.label.chars().count() > MAX_OPTION_LABEL_CHARS
            || option.description.chars().count() > MAX_OPTION_DESCRIPTION_CHARS
        {
            return Err("question option exceeds a version-one bound".to_owned());
        }
        if normalized.eq_ignore_ascii_case("other") {
            return Err("question option label `Other` is reserved".to_owned());
        }
        if !labels.insert(normalized.to_owned()) {
            return Err("question option labels must be unique after trimming".to_owned());
        }
    }
    Ok(())
}

fn validate_answers(request: &QuestionRequest, response: &QuestionResponse) -> Result<(), String> {
    let answers = &response.answers;
    let expected = request
        .questions
        .iter()
        .map(|question| question.question.as_str())
        .collect::<BTreeSet<_>>();
    let actual = answers.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("answers must contain exactly one entry for every question".to_owned());
    }
    if answers.values().any(|answer| {
        answer.trim().is_empty() || answer.chars().count() > MAX_QUESTION_ANSWER_CHARS
    }) {
        return Err("answers must be non-empty and at most 2000 characters".to_owned());
    }
    let encoded = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_QUESTION_SERIALIZED_BYTES {
        return Err("question response exceeds 65536 bytes".to_owned());
    }
    Ok(())
}

pub(crate) async fn dispatch(
    core: &CoreState,
    state: &turn::AgentTurnState,
    call: &ToolCall,
) -> ToolResult {
    if core.client_capabilities.question_request != Some(QUESTION_CAPABILITY_VERSION) {
        return ToolResult::error(&call.id, "AskUserQuestion requires client capability v1");
    }
    let questions = match parse_questions(&call.arguments) {
        Ok(questions) => questions,
        Err(error) => return ToolResult::error(&call.id, error),
    };
    let source = match state.identity() {
        turn::AgentTurnIdentity::Main => match state.events() {
            turn::TurnEventSink::Submission(id) => QuestionSource::Submission(id),
            turn::TurnEventSink::Child => {
                return ToolResult::error(&call.id, "invalid root question owner");
            }
        },
        turn::AgentTurnIdentity::Child(id) => QuestionSource::Agent(id),
    };
    let (request, mut response) = match core.questions.register(source, call.id.clone(), questions)
    {
        Ok(registered) => registered,
        Err(error) => return ToolResult::error(&call.id, error),
    };
    let request_id = request.id;
    core.emit(&CoreEvent::QuestionRequested { request });
    let mut cancellation = state.active().cancellation_receiver();
    let resolution = tokio::select! {
        resolution = &mut response => resolution.unwrap_or(QuestionResolution::Cancelled),
        _ = wait_for_cancellation(&mut cancellation) => {
            if let Some(sender) = core.questions.take_cancel(request_id) {
                core.emit(&CoreEvent::QuestionResolved { request_id });
                let _ = sender.send(QuestionResolution::Cancelled);
                QuestionResolution::Cancelled
            } else {
                response.await.unwrap_or(QuestionResolution::Cancelled)
            }
        }
    };
    match resolution {
        QuestionResolution::Answered(answers) => match serde_json::to_string(&json!({
            "answers": answers
        })) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(&call.id, error.to_string()),
        },
        QuestionResolution::Dismissed => ToolResult::success(
            &call.id,
            json!({"answers": {}, "note": DISMISSED_NOTE}).to_string(),
        ),
        QuestionResolution::Cancelled => ToolResult::error(&call.id, "tool dispatch cancelled"),
    }
}

async fn wait_for_cancellation(cancellation: &mut watch::Receiver<bool>) {
    while !*cancellation.borrow() {
        if cancellation.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        QuestionItem, QuestionOption, QuestionRegistry, QuestionResolution, QuestionResponse,
        QuestionSource, parse_questions,
    };
    use crate::core::SubmissionId;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn question_defaults_match_kimi_contract() {
        let questions = parse_questions(&json!({"questions": [{
            "question": "Choose", "options": [{"label": "A"}, {"label": "B"}]
        }]}))
        .expect("valid questions");
        assert_eq!(
            questions,
            vec![QuestionItem {
                question: "Choose".to_owned(),
                header: String::new(),
                options: vec![
                    QuestionOption {
                        label: "A".to_owned(),
                        description: String::new()
                    },
                    QuestionOption {
                        label: "B".to_owned(),
                        description: String::new()
                    },
                ],
                multi_select: false,
            }]
        );
    }

    #[test]
    fn reserved_other_is_normalized() {
        let error = parse_questions(&json!({"questions": [{
            "question": "Choose", "options": [{"label": "A"}, {"label": " other "}]
        }]}))
        .expect_err("Other must be synthesized by the client");
        assert!(error.contains("reserved"));
    }

    #[tokio::test]
    async fn accepted_answer_claim_survives_late_cancellation() {
        let registry = QuestionRegistry::new();
        let questions = parse_questions(&json!({"questions": [{
            "question": "Choose", "options": [{"label": "A"}, {"label": "B"}]
        }]}))
        .expect("valid question");
        let (request, receiver) = registry
            .register(
                QuestionSource::Submission(SubmissionId(1)),
                "call-1".to_owned(),
                questions,
            )
            .expect("register question");
        let response = QuestionResponse {
            request_id: request.id,
            answers: BTreeMap::from([("Choose".to_owned(), "A".to_owned())]),
        };

        let sender = registry.answer(&response).expect("answer wins registry");
        assert!(registry.take_cancel(request.id).is_none());
        assert!(
            sender
                .send(QuestionResolution::Answered(response.answers))
                .is_ok()
        );
        assert!(matches!(
            receiver.await.expect("resolution"),
            QuestionResolution::Answered(_)
        ));
    }

    #[tokio::test]
    async fn accepted_dismiss_claim_survives_late_cancellation() {
        let registry = QuestionRegistry::new();
        let questions = parse_questions(&json!({"questions": [{
            "question": "Choose", "options": [{"label": "A"}, {"label": "B"}]
        }]}))
        .expect("valid question");
        let (request, receiver) = registry
            .register(
                QuestionSource::Submission(SubmissionId(1)),
                "call-1".to_owned(),
                questions,
            )
            .expect("register question");

        let sender = registry.dismiss(request.id).expect("dismiss wins registry");
        assert!(registry.take_cancel(request.id).is_none());
        assert!(sender.send(QuestionResolution::Dismissed).is_ok());
        assert!(matches!(
            receiver.await.expect("resolution"),
            QuestionResolution::Dismissed
        ));
    }
}
