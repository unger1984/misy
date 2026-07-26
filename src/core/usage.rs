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
    /// The plugin owns endpoint selection and provider-specific parsing. The core only negotiates
    /// usage capability version 1, supplies opaque credentials, and validates its strict result.
    ///
    /// # Errors
    ///
    /// Returns an error when the capability is unavailable, credentials are absent, refresh or
    /// transport fails, the request exceeds its deadline, or the normalized report is malformed.
    ///
    /// # Panics
    ///
    /// Panics if a provider authentication mutex was poisoned by an earlier core-thread panic.
    pub fn usage(&self, model: &ModelRef) -> Result<UsageReport, CoreError> {
        self.ensure_running()?;
        let package = self
            .inner
            .catalog
            .get(model.provider.as_str())
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
        let operation = self.auth_operation(&model.provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.refresh_expiring_credentials_locked(&model.provider)?;
        if self.inner.credential_store.load(&model.provider)?.is_none() {
            return Err(CoreError::ProviderNotAuthenticated(model.provider.clone()));
        }
        let result = self
            .usage_provider_request(model)
            .map_err(sanitize_usage_error)?;
        UsageReport::parse_provider_value(result).map_err(CoreError::InvalidUsage)
    }

    fn usage_provider_request(&self, model: &ModelRef) -> Result<serde_json::Value, CoreError> {
        let mut params = json!({
            "provider_id": model.provider.as_str(),
            "model_id": model.model.as_str(),
        });
        if let Some(credentials) = self.inner.credential_store.load(&model.provider)? {
            params["credentials"] = credentials;
        }
        self.inner
            .host
            .request_with_timeout(&model.provider, USAGE_METHOD, params, USAGE_REQUEST_TIMEOUT)
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
