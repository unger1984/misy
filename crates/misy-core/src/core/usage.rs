//! Account-limit capability orchestration with provider-specific details kept out of the core.

use super::{CoreError, MisyCore};
use crate::{
    ModelRef, ProviderError, USAGE_CAPABILITY, USAGE_CAPABILITY_VERSION, USAGE_METHOD, UsageReport,
};
use serde_json::json;
use std::time::Duration;

const USAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl MisyCore {
    /// Fetches account-limit usage from the provider owning `model`.
    ///
    /// The plugin owns endpoint selection and parsing. The core negotiates capability version 1,
    /// supplies opaque credentials, and validates its strict normalized result.
    ///
    /// # Errors
    ///
    /// Returns an error when capability, authentication, transport, or report validation fails.
    pub async fn usage(&self, model: &ModelRef) -> Result<UsageReport, CoreError> {
        self.inner.state.ensure_running()?;
        let package = self
            .inner
            .state
            .catalog
            .get(&model.provider)
            .ok_or_else(|| ProviderError::UnknownProvider(model.provider.as_str().to_owned()))?;
        if !package
            .manifest()
            .supports_capability(USAGE_CAPABILITY, USAGE_CAPABILITY_VERSION)
        {
            return Err(CoreError::UnsupportedCapability {
                provider: model.provider.clone(),
                capability: USAGE_CAPABILITY.to_owned(),
                version: USAGE_CAPABILITY_VERSION,
            });
        }
        self.inner
            .state
            .refresh_expiring_credentials(&model.provider)
            .await?;
        if self
            .inner
            .state
            .load_credentials(&model.provider)
            .await?
            .is_none()
        {
            return Err(CoreError::ProviderNotAuthenticated(model.provider.clone()));
        }
        let mut result = self
            .usage_provider_request(model)
            .await
            .map_err(sanitize_usage_error)?;
        // Providers report credentials rotated by a silent refresh in the result; persist them
        // through the shared store path, which also strips the field so the strict report
        // parser below never sees it.
        if result.get("credentials").is_some() {
            self.inner
                .state
                .store_credentials(&model.provider, &mut result, true)
                .await?;
        }
        UsageReport::parse_provider_value(result).map_err(CoreError::InvalidUsage)
    }

    async fn usage_provider_request(
        &self,
        model: &ModelRef,
    ) -> Result<serde_json::Value, CoreError> {
        let mut params = json!({
            "provider_id": model.provider.as_str(),
            "model_id": model.model.as_str(),
        });
        if let Some(credentials) = self.inner.state.load_credentials(&model.provider).await? {
            params["credentials"] = credentials;
        }
        self.inner
            .state
            .host
            .request_with_timeout(&model.provider, USAGE_METHOD, params, USAGE_REQUEST_TIMEOUT)
            .await
            .map_err(CoreError::Provider)
    }
}

fn sanitize_usage_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Provider(ProviderError::Remote { provider, code, .. }) => {
            CoreError::Provider(ProviderError::Remote {
                provider,
                code,
                message: "usage request failed".to_owned(),
                data: None,
            })
        }
        error => error,
    }
}
