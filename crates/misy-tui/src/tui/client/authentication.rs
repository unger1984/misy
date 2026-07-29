//! Browser and device authentication completion for the terminal client.

use super::{BrowserHandoff, ProviderOperationResult, TuiClient, TuiError};
use crate::tui::{
    auth_flow::{AuthFlow, parse_auth_flow},
    auth_prompt::AuthPromptSubmission,
    state::{OperationScope, ProviderOperationKind},
};
use misy_core::ProviderId;
use serde_json::{Value, json};

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn open_authorization(
        &mut self,
        provider: ProviderId,
        method: String,
        auth: &Value,
    ) -> Result<(), TuiError> {
        match parse_auth_flow(auth).map_err(TuiError::InvalidAuthStart)? {
            AuthFlow::Browser { url, session } => {
                self.open_browser_auth(provider, method, &url, session, None)
            }
            AuthFlow::Device {
                url,
                user_code,
                session,
                ..
            } => self.open_browser_auth(provider, method, &url, session, Some(user_code)),
            AuthFlow::None => {
                self.refresh_provider_choices();
                self.state.add_info(format!(
                    "{} does not require authentication",
                    self.provider_display_name(&provider)
                ));
                Ok(())
            }
            AuthFlow::Prompt { fields, session } => {
                let display_name = self
                    .state
                    .provider_names
                    .get(&provider)
                    .cloned()
                    .unwrap_or_else(|| misy_core::ProviderDisplayName::new(provider.as_str()));
                self.state
                    .open_auth_prompt(provider, display_name, method, session, fields);
                Ok(())
            }
        }
    }

    fn open_browser_auth(
        &mut self,
        provider: ProviderId,
        method: String,
        url: &str,
        session: Value,
        device_code: Option<String>,
    ) -> Result<(), TuiError> {
        self.browser.open(url).map_err(TuiError::Browser)?;
        self.state.set_provider_operation(
            provider.clone(),
            ProviderOperationKind::Complete,
            device_code.clone(),
        );
        let detail = device_code.map_or_else(String::new, |code| format!(" with code {code}"));
        self.state.add_info(format!(
            "authorization opened for {}{detail}",
            self.provider_display_name(&provider)
        ));
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let task_provider = provider.clone();
        let task = tokio::spawn(async move {
            let result = core
                .complete_auth(&provider, session, json!({}))
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            // A closed channel means the client is gone, so the result has nowhere to land.
            let _ = sender.send(ProviderOperationResult::Complete(
                provider,
                method,
                ProviderOperationKind::Complete,
                result,
            ));
        });
        self.track_auth_task(
            task_provider,
            ProviderOperationKind::Complete,
            task.abort_handle(),
        );
        Ok(())
    }

    pub(super) fn complete_prompt_auth(&mut self, submission: AuthPromptSubmission) {
        let AuthPromptSubmission {
            provider,
            method,
            session,
            completion,
            secret_values,
        } = submission;
        self.state.set_provider_operation(
            provider.clone(),
            ProviderOperationKind::PromptComplete,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        let task_provider = provider.clone();
        let task = tokio::spawn(async move {
            let result = core
                .complete_auth(&provider, session, completion)
                .await
                .map(|_| ())
                .map_err(|error| redact_secrets(&error.to_string(), &secret_values));
            let _ = sender.send(ProviderOperationResult::Complete(
                provider,
                method,
                ProviderOperationKind::PromptComplete,
                result,
            ));
        });
        self.track_auth_task(
            task_provider,
            ProviderOperationKind::PromptComplete,
            task.abort_handle(),
        );
    }

    pub(super) fn cancel_active_authentication(&mut self) -> bool {
        let Some((OperationScope::Provider(provider), kind)) =
            self.state.provider_operation.clone()
        else {
            return false;
        };
        if !matches!(
            kind,
            ProviderOperationKind::Start
                | ProviderOperationKind::Complete
                | ProviderOperationKind::PromptComplete
        ) {
            return false;
        }
        let Some((task_provider, task_kind, task)) = self.auth_task.take() else {
            return false;
        };
        if task_provider != provider || task_kind != kind {
            self.auth_task = Some((task_provider, task_kind, task));
            return false;
        }
        task.abort();
        self.cancel_authentication(provider, Some(kind));
        true
    }

    pub(super) fn cancel_authentication(
        &mut self,
        provider: ProviderId,
        operation: Option<ProviderOperationKind>,
    ) {
        if let Some(kind) = operation {
            self.state.finish_provider_operation(&provider, kind);
        }
        self.state.set_provider_operation(
            provider.clone(),
            ProviderOperationKind::CancelAuth,
            None,
        );
        let core = self.core.clone();
        let sender = self.operation_sender.clone();
        tokio::spawn(async move {
            let result = core
                .cancel_authentication(&provider)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(ProviderOperationResult::CancelAuth(provider, result));
        });
    }
}

fn redact_secrets(message: &str, secrets: &[String]) -> String {
    secrets.iter().fold(message.to_owned(), |redacted, secret| {
        redacted.replace(secret, "[redacted]")
    })
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    #[test]
    fn exact_secret_redaction_handles_overlapping_values() {
        assert_eq!(
            redact_secrets(
                "rejected sk-long and sk",
                &["sk-long".to_owned(), "sk".to_owned()]
            ),
            "rejected [redacted] and [redacted]"
        );
    }
}
