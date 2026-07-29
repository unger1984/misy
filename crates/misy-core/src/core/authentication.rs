//! Credential persistence, refresh serialization, and authentication event emission.

use super::{CoreError, CoreEvent, CoreState, MisyCore};
use crate::{ProviderAuthState, ProviderId, config::CredentialStore, providers::ProviderCatalog};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn credential_states(
    catalog: &ProviderCatalog,
    credential_store: &CredentialStore,
) -> BTreeMap<ProviderId, ProviderCredentialState> {
    catalog
        .packages()
        .map(|package| {
            let provider = package.manifest().id.clone();
            // Startup cache failures must not make discovery fail; each operation validates again.
            let credentials = credential_store.load(&provider).ok().flatten();
            let authenticated = credentials.is_some();
            let credential_method = credentials.as_ref().and_then(credential_method);
            (
                provider.clone(),
                ProviderCredentialState {
                    auth: ProviderAuthState {
                        id: provider,
                        authenticated,
                        credential_method,
                    },
                    epoch: 0,
                },
            )
        })
        .collect()
}

pub(super) fn credential_method(credentials: &Value) -> Option<String> {
    credentials
        .get("type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

/// One provider's credential presence and its mutation epoch, held as a single record so the
/// model-cache gate and the client-visible authentication state cannot diverge.
pub(crate) struct ProviderCredentialState {
    pub(super) auth: ProviderAuthState,
    epoch: u64,
}

/// How a credential-state mutation affects the recorded credential method.
pub(super) enum CredentialMethodChange {
    /// The mutation carries no credential record, so the recorded method stays untouched.
    Preserve,
    /// The mutation stores or removes a credential record and replaces the recorded method.
    Set(Option<String>),
}

/// A point-in-time capture of one provider's credential epoch, derived from the shared
/// [`ProviderCredentialState`] record; `present` always mirrors `auth.authenticated`.
#[derive(Clone, Copy)]
pub(crate) struct CredentialEpoch {
    pub(super) value: u64,
    pub(super) present: bool,
}

impl MisyCore {
    /// Starts a provider-owned authentication flow without exposing credentials.
    ///
    /// A provider that returns the exact `none` flow kind has authenticated immediately. Its
    /// state is retained in memory only because no opaque credential record was supplied.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is shut down or the provider cannot start authentication.
    pub async fn start_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        self.inner.state.ensure_running()?;
        let package =
            self.inner.state.catalog.get(provider).ok_or_else(|| {
                crate::ProviderError::UnknownProvider(provider.as_str().to_owned())
            })?;
        let method = package
            .manifest()
            .auth_methods
            .first()
            .map(|method| method.id.clone())
            .ok_or_else(|| CoreError::UnsupportedAuthMethod {
                provider: provider.clone(),
                method: "default".to_owned(),
            })?;
        self.start_auth_with_method(provider, &method).await
    }

    /// Starts one provider-declared authentication method without exposing credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when `method` is not declared, the core is shut down, or startup fails.
    pub async fn start_auth_with_method(
        &self,
        provider: &ProviderId,
        method: &str,
    ) -> Result<Value, CoreError> {
        self.inner.state.ensure_running()?;
        let package =
            self.inner.state.catalog.get(provider).ok_or_else(|| {
                crate::ProviderError::UnknownProvider(provider.as_str().to_owned())
            })?;
        if !package
            .manifest()
            .auth_methods
            .iter()
            .any(|candidate| candidate.id == method)
        {
            return Err(CoreError::UnsupportedAuthMethod {
                provider: provider.clone(),
                method: method.to_owned(),
            });
        }
        let response = self
            .inner
            .state
            .provider_request(
                provider,
                "auth.start",
                serde_json::json!({ "method": method }),
            )
            .await?;
        if response.get("kind").and_then(Value::as_str) == Some("none") {
            // The `none` flow authenticates with no credential record at all — the deliberate
            // analog of oh-my-pi's kNoAuth sentinel for providers that need no authorization.
            // This is the only path allowed to record authentication without credentials.
            self.inner.state.record_authentication(
                provider,
                true,
                CredentialMethodChange::Set(Some(method.to_owned())),
            );
            self.emit(&CoreEvent::AuthenticationChanged {
                provider: provider.clone(),
                authenticated: true,
            });
        }
        Ok(self.sanitize_auth_response(response))
    }

    /// Reports the cached authentication state for `provider`.
    ///
    /// # Errors
    ///
    /// The cache is initialized during core construction and updated after successful
    /// authentication operations, so this method never reads credential files.
    pub async fn has_credentials(&self, provider: &ProviderId) -> Result<bool, CoreError> {
        Ok(self
            .inner
            .state
            .authentication_state(provider)
            .authenticated)
    }

    /// Returns the cached authentication method without starting a provider process.
    ///
    /// # Errors
    ///
    /// The cache is initialized during core construction and updated after successful
    /// authentication operations, so this method never reads credential files.
    pub async fn credential_method(
        &self,
        provider: &ProviderId,
    ) -> Result<Option<String>, CoreError> {
        Ok(self
            .inner
            .state
            .authentication_state(provider)
            .credential_method)
    }

    /// Completes a provider-owned authentication flow and persists returned credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when completion or private credential persistence fails, or when a
    /// credential-bearing flow's result omits the credentials protocol v2 requires.
    pub async fn complete_auth(
        &self,
        provider: &ProviderId,
        session: Value,
        completion: Value,
    ) -> Result<Value, CoreError> {
        self.inner.state.ensure_running()?;
        let operation = self.inner.state.auth_operation(provider);
        let _operation = operation.lock().await;
        let mut result = self
            .inner
            .state
            .provider_request(
                provider,
                "auth.complete",
                serde_json::json!({ "session": session, "completion": completion }),
            )
            .await?;
        self.inner
            .state
            .require_auth_credentials(provider, &result, "auth.complete")
            .await?;
        self.inner
            .state
            .store_credentials(provider, &mut result, true)
            .await?;
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: true,
        });
        Ok(self.sanitize_auth_response(result))
    }

    /// Cancels an in-flight interactive authentication attempt for `provider`.
    ///
    /// Authentication requests have no protocol-level cancellation method, so cancellation
    /// terminates the provider process. A later provider operation starts a clean replacement.
    ///
    /// # Errors
    ///
    /// Returns an error when the core is already shut down.
    pub async fn cancel_authentication(&self, provider: &ProviderId) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        self.inner
            .state
            .host
            .fail_provider(provider, "provider authentication cancelled".to_owned())
            .await;
        Ok(())
    }

    /// Refreshes provider credentials and persists the provider's returned replacement.
    ///
    /// # Errors
    ///
    /// Returns an error when provider refresh or private credential persistence fails, or when
    /// the result omits the credentials protocol v2 requires outside the `none` flow.
    pub async fn refresh_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        self.inner.state.ensure_running()?;
        self.inner.state.refresh_auth(provider).await
    }

    /// Logs out a provider and removes its stored opaque credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when logout or credential removal fails.
    pub async fn logout(&self, provider: &ProviderId) -> Result<(), CoreError> {
        self.inner.state.ensure_running()?;
        let operation = self.inner.state.auth_operation(provider);
        let _operation = operation.lock().await;
        self.inner
            .state
            .provider_request(provider, "auth.logout", serde_json::json!({}))
            .await?;
        self.inner.state.remove_credentials(provider).await?;
        self.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: false,
        });
        Ok(())
    }
}

impl CoreState {
    pub(super) async fn refresh_expiring_credentials(
        &self,
        provider: &ProviderId,
    ) -> Result<(), CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation.lock().await;
        self.refresh_expiring_credentials_locked(provider).await
    }

    pub(super) async fn refresh_auth(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let operation = self.auth_operation(provider);
        let _operation = operation.lock().await;
        self.refresh_auth_locked(provider).await
    }

    async fn refresh_auth_locked(&self, provider: &ProviderId) -> Result<Value, CoreError> {
        let mut result = self
            .provider_request(provider, "auth.refresh", serde_json::json!({}))
            .await?;
        self.require_auth_credentials(provider, &result, "auth.refresh")
            .await?;
        self.store_credentials(provider, &mut result, true).await?;
        self.subscribers.emit(&CoreEvent::AuthenticationChanged {
            provider: provider.clone(),
            authenticated: true,
        });
        Ok(super::strip_credentials(result))
    }

    async fn refresh_expiring_credentials_locked(
        &self,
        provider: &ProviderId,
    ) -> Result<(), CoreError> {
        let credentials = self.load_credentials(provider).await?;
        let Some(expires_at) = credentials
            .as_ref()
            .and_then(|value| value.get("expires_at"))
            .and_then(Value::as_u64)
        else {
            return Ok(());
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| CoreError::InvalidUsage(error.to_string()))?
            .as_millis();
        if u128::from(expires_at) <= now.saturating_add(60_000) {
            self.refresh_auth_locked(provider).await?;
        }
        Ok(())
    }

    pub(super) fn auth_operation(&self, provider: &ProviderId) -> Arc<tokio::sync::Mutex<()>> {
        let mut operations = self
            .auth_operations
            .lock()
            .expect("auth operations mutex must not be poisoned");
        Arc::clone(
            operations
                .entry(provider.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    pub(super) fn credential_epoch(&self, provider: &ProviderId) -> CredentialEpoch {
        let states = self
            .credential_states
            .lock()
            .expect("credential state mutex must not be poisoned");
        states.get(provider).map_or(
            CredentialEpoch {
                value: 0,
                present: false,
            },
            |state| CredentialEpoch {
                value: state.epoch,
                present: state.auth.authenticated,
            },
        )
    }

    pub(super) fn authentication_state(&self, provider: &ProviderId) -> ProviderAuthState {
        self.credential_states
            .lock()
            .expect("credential state mutex must not be poisoned")
            .get(provider)
            .map(|state| state.auth.clone())
            .unwrap_or_else(|| ProviderAuthState {
                id: provider.clone(),
                authenticated: false,
                credential_method: None,
            })
    }

    /// Records one authentication outcome and advances the provider's credential epoch.
    ///
    /// This is the only writer of [`ProviderCredentialState`]: credential presence, the client
    /// projection, and the model-cache epoch share one record, so a status query cannot leave the
    /// cache gate believing credentials that [`MisyCore::has_credentials`] denies. The epoch grows
    /// on every call, which also invalidates model listings captured against older state.
    pub(super) fn record_authentication(
        &self,
        provider: &ProviderId,
        authenticated: bool,
        credential_method: CredentialMethodChange,
    ) -> u64 {
        let mut states = self
            .credential_states
            .lock()
            .expect("credential state mutex must not be poisoned");
        let state = states
            .entry(provider.clone())
            .or_insert_with(|| ProviderCredentialState {
                auth: ProviderAuthState {
                    id: provider.clone(),
                    authenticated: false,
                    credential_method: None,
                },
                epoch: 0,
            });
        state.auth.authenticated = authenticated;
        if let CredentialMethodChange::Set(method) = credential_method {
            state.auth.credential_method = method;
        }
        state.epoch = state.epoch.wrapping_add(1);
        state.epoch
    }

    /// Enforces the protocol v2 contract that an `auth.complete`/`auth.refresh` result carries
    /// replacement `credentials`.
    ///
    /// The exception is a provider authenticated through the `none` flow: it legitimately holds
    /// no credential record (the kNoAuth analog), so a credential-less result is not a violation
    /// for it. Without this gate a plugin that silently omits credentials would still be marked
    /// authenticated while [`MisyCore::has_credentials`] reports true and every credentialed
    /// operation then fails downstream.
    async fn require_auth_credentials(
        &self,
        provider: &ProviderId,
        result: &Value,
        method: &str,
    ) -> Result<(), CoreError> {
        if result.get("credentials").is_some() {
            return Ok(());
        }
        let state = self.authentication_state(provider);
        if state.authenticated && self.load_credentials(provider).await?.is_none() {
            return Ok(());
        }
        Err(crate::ProviderError::Protocol {
            provider: provider.as_str().to_owned(),
            message: format!("{method} succeeded without the required credentials"),
        }
        .into())
    }

    pub(super) async fn store_credentials(
        &self,
        provider: &ProviderId,
        response: &mut Value,
        present: bool,
    ) -> Result<(), CoreError> {
        let _credentials = self.credential_operations.lock().await;
        self.store_credentials_locked(provider, response, present)
            .await
    }

    /// Stores a chat credential rotation only when it was produced against the current epoch.
    ///
    /// The epoch comparison shares the credential mutation lock with file writes, so a newer
    /// refresh or logout wins over a late chat response without restoring stale credentials.
    pub(super) async fn store_credentials_if_epoch(
        &self,
        provider: &ProviderId,
        expected: CredentialEpoch,
        response: &mut Value,
        present: bool,
    ) -> Result<(), CoreError> {
        let _credentials = self.credential_operations.lock().await;
        if self.credential_epoch(provider).value != expected.value {
            if let Some(object) = response.as_object_mut() {
                object.remove("credentials");
            }
            return Ok(());
        }
        self.store_credentials_locked(provider, response, present)
            .await
    }

    async fn store_credentials_locked(
        &self,
        provider: &ProviderId,
        response: &mut Value,
        present: bool,
    ) -> Result<(), CoreError> {
        // A credential-less mutation (the `none` flow) carries no record to derive a method
        // from, so the previously recorded method stays untouched.
        let method_change = match response.get("credentials") {
            Some(credentials) => CredentialMethodChange::Set(credential_method(credentials)),
            None => CredentialMethodChange::Preserve,
        };
        if let Some(credentials) = response.get("credentials").cloned() {
            let store = self.credential_store.clone();
            let provider = provider.clone();
            tokio::task::spawn_blocking(move || store.save(&provider, credentials))
                .await
                .map_err(|error| CoreError::Runtime(error.to_string()))??;
            if let Some(object) = response.as_object_mut() {
                object.remove("credentials");
            }
        }
        let epoch = self.record_authentication(provider, present, method_change);
        let write = self.model_cache.stage_remove(provider, epoch);
        if let Some(write) = write {
            let cache = Arc::clone(&self.model_cache);
            // Cache persistence is best effort; the credential change itself already succeeded.
            let _ = tokio::task::spawn_blocking(move || cache.persist(write)).await;
        }
        Ok(())
    }

    async fn remove_credentials(&self, provider: &ProviderId) -> Result<(), CoreError> {
        let _credentials = self.credential_operations.lock().await;
        let store = self.credential_store.clone();
        let provider_for_file = provider.clone();
        tokio::task::spawn_blocking(move || store.remove(&provider_for_file))
            .await
            .map_err(|error| CoreError::Runtime(error.to_string()))??;
        let epoch = self.record_authentication(provider, false, CredentialMethodChange::Set(None));
        if let Some(write) = self.model_cache.stage_remove(provider, epoch) {
            let cache = Arc::clone(&self.model_cache);
            // Cache persistence is best effort; the credential removal itself already succeeded.
            let _ = tokio::task::spawn_blocking(move || cache.persist(write)).await;
        }
        Ok(())
    }
}
