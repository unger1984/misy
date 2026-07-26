//! Credential lifecycle and cache invalidation for the headless core.

use super::{CoreError, CoreEvent, CredentialEpoch, MisyCore};
use crate::ProviderId;
use serde_json::Value;
use std::sync::{Arc, Mutex};

impl MisyCore {
    /// Completes a provider-owned authentication flow and persists returned credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when provider completion or private credential persistence fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    #[allow(clippy::needless_pass_by_value)]
    pub fn complete_auth(
        &self,
        provider: &ProviderId,
        session: Value,
        completion: Value,
    ) -> Result<Value, CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        let mut result = self.provider_request(
            provider,
            "auth.complete",
            serde_json::json!({ "session": session, "completion": completion }),
        )?;
        let cache_write = {
            let _credentials = self
                .inner
                .credential_operations
                .lock()
                .expect("credential operation mutex must not be poisoned");
            self.store_returned_credentials(provider, &mut result)?;
            let epoch = self.advance_credential_epoch_locked(provider, true);
            self.inner.model_cache.stage_remove(provider, epoch)
        };
        if let Some(cache_write) = cache_write {
            let _ = self.inner.model_cache.persist(cache_write);
        }
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: true,
        });
        Ok(self.sanitize_auth_response(result))
    }

    /// Refreshes provider credentials and persists the provider's returned replacement.
    ///
    /// # Errors
    ///
    /// Returns an error when provider refresh or private credential persistence fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    pub fn refresh_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.refresh_auth_locked(provider)
    }

    pub(super) fn refresh_auth_locked(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let mut result = self.provider_request(provider, "auth.refresh", serde_json::json!({}))?;
        let cache_write = {
            let _credentials = self
                .inner
                .credential_operations
                .lock()
                .expect("credential operation mutex must not be poisoned");
            self.store_returned_credentials(provider, &mut result)?;
            let epoch = self.advance_credential_epoch_locked(provider, true);
            self.inner.model_cache.stage_remove(provider, epoch)
        };
        if let Some(cache_write) = cache_write {
            let _ = self.inner.model_cache.persist(cache_write);
        }
        Ok(self.sanitize_auth_response(result))
    }

    /// Logs out a provider and removes its stored opaque credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when logout or credential removal fails.
    ///
    /// # Panics
    ///
    /// Panics if an internal authentication or credential-operation mutex is poisoned by an
    /// earlier core-thread panic.
    pub fn logout(&self, provider: &ProviderId) -> Result<(), CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation
            .lock()
            .expect("provider auth operation mutex must not be poisoned");
        self.provider_request(provider, "auth.logout", serde_json::json!({}))?;
        let cache_write = {
            let _credentials = self
                .inner
                .credential_operations
                .lock()
                .expect("credential operation mutex must not be poisoned");
            self.inner.credential_store.remove(provider)?;
            let epoch = self.advance_credential_epoch_locked(provider, false);
            self.inner.model_cache.stage_remove(provider, epoch)
        };
        if let Some(cache_write) = cache_write {
            let _ = self.inner.model_cache.persist(cache_write);
        }
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: false,
        });
        Ok(())
    }

    pub(super) fn credential_epoch(&self, provider: &ProviderId) -> CredentialEpoch {
        let _credentials = self
            .inner
            .credential_operations
            .lock()
            .expect("credential operation mutex must not be poisoned");
        self.credential_epoch_locked(provider)
    }

    pub(super) fn credential_epoch_locked(&self, provider: &ProviderId) -> CredentialEpoch {
        self.inner
            .credential_epochs
            .lock()
            .expect("credential epoch mutex must not be poisoned")
            .get(provider.as_str())
            .copied()
            .unwrap_or(CredentialEpoch {
                value: 0,
                present: false,
            })
    }

    fn advance_credential_epoch_locked(&self, provider: &ProviderId, present: bool) -> u64 {
        let mut epochs = self
            .inner
            .credential_epochs
            .lock()
            .expect("credential epoch mutex must not be poisoned");
        let epoch = epochs
            .entry(provider.as_str().to_owned())
            .or_insert(CredentialEpoch {
                value: 0,
                present: false,
            });
        epoch.value = epoch.value.wrapping_add(1);
        epoch.present = present;
        epoch.value
    }

    pub(super) fn auth_operation(&self, provider: &ProviderId) -> Arc<Mutex<()>> {
        let mut operations = self
            .inner
            .auth_operations
            .lock()
            .expect("auth operations mutex must not be poisoned");
        Arc::clone(
            operations
                .entry(provider.as_str().to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    fn store_returned_credentials(
        &self,
        provider: &ProviderId,
        response: &mut Value,
    ) -> Result<(), CoreError> {
        if let Some(credentials) = response.get("credentials").cloned() {
            self.inner.credential_store.save(provider, credentials)?;
            if let Some(object) = response.as_object_mut() {
                object.remove("credentials");
            }
        }
        Ok(())
    }
}
