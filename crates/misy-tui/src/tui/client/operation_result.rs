//! Background provider operations and the dispatch of their results.
//!
//! Every method here either spawns a Tokio task that ends in a
//! [`ProviderOperationResult`] or applies such a result back to the UI state,
//! so the spawn site and its completion branch live next to each other.

use super::{BrowserHandoff, ProviderOperationResult, SubmissionRequest, TuiClient, TuiError};
use crate::tui::composer_attachment::ComposerDraft;
use crate::tui::state::ProviderOperationKind;
use misy_core::{
    AvailableModels, CoreError, Message, MisyCore, ModelProfile, ModelRef, ProviderId,
    SubmissionId, UsageReport,
};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn start_compaction(&mut self, focus: Option<String>) {
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .compact(focus.as_deref())
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::Compact(result));
        });
    }

    pub(super) fn show_thinking(&mut self) {
        let snapshot = self.core.snapshot();
        let Some(selected) = snapshot.selected_model else {
            self.state.add_error(CoreError::NoModelSelected);
            return;
        };
        let Some(model) = self
            .cached_models
            .models
            .iter()
            .find(|model| model.model == selected)
        else {
            self.state
                .add_error("reasoning metadata is unavailable; refresh /model first");
            return;
        };
        if model
            .thinking
            .as_ref()
            .is_none_or(|thinking| thinking.levels.is_empty())
        {
            self.state
                .add_error("the selected model does not expose reasoning levels");
            return;
        }
        self.state
            .open_thinking_only(model, snapshot.selected_thinking.as_deref());
    }
    pub(super) fn show_usage(&mut self) -> Result<(), TuiError> {
        let Some(model) = self.core.snapshot().selected_model else {
            self.state.add_error(CoreError::NoModelSelected);
            return Ok(());
        };
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn({
            let requested_model = model;
            async move {
                let result = core
                    .usage(&requested_model)
                    .await
                    .map_err(|error| error.to_string());
                // A closed channel means the client is gone, so the result has nowhere to land.
                let _ = sender.send(ProviderOperationResult::Usage(requested_model, result));
            }
        });
        Ok(())
    }

    pub(super) fn submit_prompt(&mut self, prompt: String) {
        let request = SubmissionRequest {
            draft: ComposerDraft {
                text: prompt,
                images: Vec::new(),
            },
            clear_composer: false,
            history_text: None,
            history_snapshot: None,
        };
        if self.submission_sender.send(request).is_err() {
            self.state
                .add_error("prompt submission worker stopped unexpectedly");
        }
    }

    pub(super) fn start_auth(&mut self, provider: ProviderId, method: String) {
        self.state
            .set_provider_operation(provider.clone(), ProviderOperationKind::Start, None);
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let task_provider = provider.clone();
        let task = tokio::spawn(async move {
            let result = core
                .start_auth_with_method(&provider, &method)
                .await
                .map_err(|error| error.to_string());
            // A closed channel means the client is gone, so the result has nowhere to land.
            let _ = sender.send(ProviderOperationResult::Start(provider, method, result));
        });
        self.track_auth_task(
            task_provider,
            ProviderOperationKind::Start,
            task.abort_handle(),
        );
    }

    pub(super) fn logout(&mut self, provider: ProviderId) {
        self.state
            .set_provider_operation(provider.clone(), ProviderOperationKind::Logout, None);
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .logout(&provider)
                .await
                .map_err(|error| error.to_string());
            // A closed channel means the client is gone, so the result has nowhere to land.
            let _ = sender.send(ProviderOperationResult::Logout(provider, result));
        });
    }

    pub(super) fn select_model(&mut self, model: ModelRef) {
        self.state.set_provider_operation(
            model.provider.clone(),
            ProviderOperationKind::SelectModel,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let provider = model.provider.clone();
        tokio::spawn(async move {
            let result = core
                .select_model(model.clone())
                .await
                .map(|()| model)
                .map_err(|error| error.to_string());
            // A closed channel means the client is gone, so the result has nowhere to land.
            let _ = sender.send(ProviderOperationResult::SelectModel(provider, result));
        });
    }

    pub(super) fn select_profile(&mut self, profile: ModelProfile) {
        self.state.set_provider_operation(
            profile.model.provider.clone(),
            ProviderOperationKind::SelectModel,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let provider = profile.model.provider.clone();
        let model = profile.model.clone();
        tokio::spawn(async move {
            let result = core
                .select_profile(profile)
                .await
                .map(|()| model)
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::SelectModel(provider, result));
        });
    }

    pub(super) fn select_thinking(&mut self, profile: ModelProfile) {
        self.state.set_provider_operation(
            profile.model.provider.clone(),
            ProviderOperationKind::SelectModel,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let provider = profile.model.provider.clone();
        let model = profile.model;
        tokio::spawn(async move {
            let result = core
                .select_thinking(profile.thinking)
                .await
                .map(|()| model)
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::SelectModel(provider, result));
        });
    }

    pub(super) fn apply_operation_result(&mut self, result: ProviderOperationResult) {
        match result {
            ProviderOperationResult::Models(generation, result) => {
                self.apply_models_result(generation, result);
            }
            ProviderOperationResult::Start(provider, method, result) => {
                self.apply_start_result(provider, method, result);
            }
            ProviderOperationResult::Complete(provider, _method, kind, result) => {
                self.apply_complete_result(&provider, kind, result);
            }
            ProviderOperationResult::CancelAuth(provider, result) => {
                self.apply_cancel_auth_result(&provider, result);
            }
            ProviderOperationResult::Logout(provider, result) => {
                self.apply_logout_result(&provider, result);
            }
            ProviderOperationResult::SelectModel(provider, result) => {
                self.apply_select_model_result(&provider, result);
            }
            ProviderOperationResult::Usage(model, result) => {
                self.apply_usage_result(&model, result);
            }
            ProviderOperationResult::Compact(result) => match result {
                Ok(_) => self.refresh_core_projection(),
                Err(error) => self.state.add_error(error),
            },
            ProviderOperationResult::Submit(request, result) => {
                self.apply_submit_result(request, result);
            }
            ProviderOperationResult::ActivityOutput(_id, output) => {
                self.activity_output_pending = false;
                if let Some(output) = output {
                    self.state.set_activity_output(output);
                }
            }
            ProviderOperationResult::AgentTranscript(_id, transcript) => {
                self.activity_output_pending = false;
                match transcript {
                    Ok(transcript) => self.state.set_agent_transcript(transcript),
                    Err(error) => self.state.add_error(error),
                }
            }
            ProviderOperationResult::DiscardAgents(result) => match result {
                Ok(()) => {
                    let pending = self.pending_session_switch.take();
                    let outcome = match pending {
                        Some(super::PendingSessionSwitch::New) => self.start_new_session(),
                        Some(super::PendingSessionSwitch::Resume(id)) => self.resume_session(&id),
                        None => Ok(()),
                    };
                    if let Err(error) = outcome {
                        self.state.add_error(error);
                    }
                }
                Err(error) => self.state.add_error(error),
            },
        }
    }

    fn apply_models_result(&mut self, generation: u64, result: Result<AvailableModels, String>) {
        // A newer refresh supersedes any load that still carries an older generation, so the
        // late result must not touch the cache or surface its error.
        if generation != self.model_refresh_generation {
            return;
        }
        match result {
            Ok(available) => {
                self.cached_models = available.clone();
                self.state.finish_models(available, &self.provider_names);
            }
            Err(error) => {
                if self.state.finish_model_catalog_operation() {
                    self.state.add_error(error);
                }
            }
        }
    }

    fn apply_start_result(
        &mut self,
        provider: ProviderId,
        method: String,
        result: Result<Value, String>,
    ) {
        if !self
            .state
            .finish_provider_operation(&provider, ProviderOperationKind::Start)
        {
            return;
        }
        self.finish_auth_task(&provider, ProviderOperationKind::Start);
        match result {
            Ok(auth) => {
                if let Err(error) = self.open_authorization(provider, method, &auth) {
                    self.state.add_error(error);
                }
            }
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_complete_result(
        &mut self,
        provider: &ProviderId,
        kind: ProviderOperationKind,
        result: Result<(), String>,
    ) {
        if !self.state.finish_provider_operation(provider, kind) {
            return;
        }
        self.finish_auth_task(provider, kind);
        match result {
            Ok(()) => self.refresh_provider_choices(),
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_cancel_auth_result(&mut self, provider: &ProviderId, result: Result<(), String>) {
        if !self
            .state
            .finish_provider_operation(provider, ProviderOperationKind::CancelAuth)
        {
            return;
        }
        match result {
            Ok(()) => {
                self.refresh_provider_choices();
                self.state.add_info(format!(
                    "authentication cancelled for {}",
                    self.provider_display_name(provider)
                ));
            }
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_logout_result(&mut self, provider: &ProviderId, result: Result<(), String>) {
        if !self
            .state
            .finish_provider_operation(provider, ProviderOperationKind::Logout)
        {
            return;
        }
        match result {
            Ok(()) => self.refresh_provider_choices(),
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_select_model_result(
        &mut self,
        provider: &ProviderId,
        result: Result<ModelRef, String>,
    ) {
        if !self
            .state
            .finish_provider_operation(provider, ProviderOperationKind::SelectModel)
        {
            return;
        }
        match result {
            Ok(model) => {
                self.refresh_core_projection();
                self.state.view = None;
                self.state
                    .add_info(format!("model: {}", model.model.as_str()));
            }
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_usage_result(&mut self, model: &ModelRef, result: Result<UsageReport, String>) {
        match result {
            Ok(report) => {
                let provider = self.provider_display_name(&model.provider).to_owned();
                for line in crate::tui::usage::format_usage_report(&provider, &report) {
                    self.state.add_info(line);
                }
            }
            Err(error) => self.state.add_error(error),
        }
    }

    fn apply_submit_result(
        &mut self,
        request: SubmissionRequest,
        result: Result<SubmissionId, String>,
    ) {
        if request.clear_composer {
            self.composer_submission_pending = false;
        }
        match result {
            Ok(_) => {
                if request.clear_composer && self.state.composer.matches_draft(&request.draft) {
                    self.state.composer.clear();
                }
                if let Some(snapshot) = request.history_snapshot {
                    let recorded = self.state.composer.record_submitted_snapshot(snapshot);
                    if recorded && let Some(text) = request.history_text {
                        self.append_prompt_history(&text);
                    }
                }
            }
            Err(error) => self.state.add_error(error),
        }
    }
}

pub(super) fn spawn_submission_worker(
    core: MisyCore,
    operation_sender: UnboundedSender<ProviderOperationResult>,
    mut submission_requests: UnboundedReceiver<SubmissionRequest>,
) {
    tokio::spawn(async move {
        while let Some(request) = submission_requests.recv().await {
            let result = core
                .submit_with_attachments(
                    Message::user(request.draft.text.clone()),
                    request.draft.images.clone(),
                )
                .await
                .map_err(|error| error.to_string());
            // A closed channel means the client is gone, so the result has nowhere to land.
            let _ = operation_sender.send(ProviderOperationResult::Submit(request, result));
        }
    });
}
