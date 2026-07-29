//! Browser and device authentication completion for the terminal client.

use super::{BrowserHandoff, ProviderOperationResult, TuiClient, TuiError};
use crate::tui::{
    auth_flow::{AuthFlow, parse_auth_flow},
    state::ProviderOperationKind,
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
            AuthFlow::Prompt { .. } => Err(TuiError::InvalidAuthStart(
                "authentication method is not supported by this client yet".to_owned(),
            )),
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
            let _ = sender.send(ProviderOperationResult::Complete(provider, method, result));
        });
        self.track_auth_task(
            task_provider,
            ProviderOperationKind::Complete,
            task.abort_handle(),
        );
        Ok(())
    }
}
