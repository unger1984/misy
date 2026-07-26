//! Credential persistence, refresh serialization, and authentication event emission.

use super::{CoreError, CoreEvent, CoreState, MisyCore};
use crate::{CredentialStore, ProviderAuthState, ProviderCatalog, ProviderId};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn credential_states(
    catalog: &ProviderCatalog,
    credential_store: &CredentialStore,
) -> BTreeMap<String, ProviderAuthState> {
    catalog
        .packages()
        .map(|package| {
            let provider = package.manifest().id.clone();
            // Startup cache failures must not make discovery fail; each operation validates again.
            let credentials = credential_store.load(&provider).ok().flatten();
            let authenticated = credentials.is_some();
            let credential_method = credentials.as_ref().and_then(credential_method);
            (
                provider.as_str().to_owned(),
                ProviderAuthState {
                    id: provider,
                    authenticated,
                    credential_method,
                },
            )
        })
        .collect()
}

pub(super) fn credential_epochs(
    credential_states: &BTreeMap<String, ProviderAuthState>,
) -> BTreeMap<String, CredentialEpoch> {
    credential_states
        .iter()
        .map(|(id, state)| {
            (
                id.clone(),
                CredentialEpoch {
                    value: 0,
                    present: state.authenticated,
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
        let package = self
            .inner
            .state
            .catalog
            .get(provider.as_str())
            .ok_or_else(|| crate::ProviderError::UnknownProvider(provider.as_str().to_owned()))?;
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
        let package = self
            .inner
            .state
            .catalog
            .get(provider.as_str())
            .ok_or_else(|| crate::ProviderError::UnknownProvider(provider.as_str().to_owned()))?;
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
            self.inner
                .state
                .update_authentication_state(provider, true, Some(method.to_owned()));
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
    /// Returns an error when completion or private credential persistence fails.
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
            .store_credentials(provider, &mut result, true)
            .await?;
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
                .entry(provider.as_str().to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    pub(super) fn credential_epoch(&self, provider: &ProviderId) -> CredentialEpoch {
        self.credential_epochs
            .lock()
            .expect("credential epoch mutex must not be poisoned")
            .get(provider.as_str())
            .copied()
            .unwrap_or(CredentialEpoch {
                value: 0,
                present: false,
            })
    }

    pub(super) fn authentication_state(&self, provider: &ProviderId) -> ProviderAuthState {
        self.auth_states
            .lock()
            .expect("authentication state mutex must not be poisoned")
            .get(provider.as_str())
            .cloned()
            .unwrap_or_else(|| ProviderAuthState {
                id: provider.clone(),
                authenticated: false,
                credential_method: None,
            })
    }

    pub(super) fn update_authentication_status(&self, provider: &ProviderId, authenticated: bool) {
        let mut states = self
            .auth_states
            .lock()
            .expect("authentication state mutex must not be poisoned");
        let state = states
            .entry(provider.as_str().to_owned())
            .or_insert_with(|| ProviderAuthState {
                id: provider.clone(),
                authenticated: false,
                credential_method: None,
            });
        state.authenticated = authenticated;
        if !authenticated {
            state.credential_method = None;
        }
    }

    pub(super) async fn store_credentials(
        &self,
        provider: &ProviderId,
        response: &mut Value,
        present: bool,
    ) -> Result<(), CoreError> {
        let _credentials = self.credential_operations.lock().await;
        let credential_method = response.get("credentials").and_then(credential_method);
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
        self.update_authentication_state(provider, present, credential_method);
        let epoch = self.advance_credential_epoch(provider, present);
        let write = self.model_cache.stage_remove(provider, epoch);
        if let Some(write) = write {
            let cache = Arc::clone(&self.model_cache);
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
        self.update_authentication_state(provider, false, None);
        let epoch = self.advance_credential_epoch(provider, false);
        if let Some(write) = self.model_cache.stage_remove(provider, epoch) {
            let cache = Arc::clone(&self.model_cache);
            let _ = tokio::task::spawn_blocking(move || cache.persist(write)).await;
        }
        Ok(())
    }

    fn advance_credential_epoch(&self, provider: &ProviderId, present: bool) -> u64 {
        let mut epochs = self
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

    pub(super) fn update_authentication_state(
        &self,
        provider: &ProviderId,
        authenticated: bool,
        credential_method: Option<String>,
    ) {
        let mut states = self
            .auth_states
            .lock()
            .expect("authentication state mutex must not be poisoned");
        states.insert(
            provider.as_str().to_owned(),
            ProviderAuthState {
                id: provider.clone(),
                authenticated,
                credential_method,
            },
        );
    }
}
