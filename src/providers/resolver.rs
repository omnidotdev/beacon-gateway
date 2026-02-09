//! Resolve per-user API keys from Gatekeeper vault

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::ApiKeys;

/// Cached API key entry
struct CachedKey {
    api_key: String,
    model_override: Option<String>,
    expires_at: Instant,
}

/// Response from Gatekeeper vault GET endpoint
#[derive(Debug, Deserialize)]
struct VaultKeyResponse {
    api_key: String,
    #[allow(dead_code)]
    provider: String,
    model_override: Option<String>,
}

/// Request body for Gatekeeper vault POST endpoint
#[derive(Debug, Serialize)]
struct StoreKeyRequest {
    api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

/// Response from Gatekeeper vault POST endpoint
#[derive(Debug, Deserialize)]
pub struct StoreKeyResponse {
    pub success: bool,
    pub message: String,
}

/// Response from Gatekeeper vault DELETE endpoint
#[derive(Debug, Deserialize)]
pub struct DeleteKeyResponse {
    pub success: bool,
    pub message: String,
}

/// Provider status from Gatekeeper vault
#[derive(Debug, Clone)]
pub struct ConfiguredProvider {
    pub provider: String,
    pub has_user_key: bool,
}

/// Resolve per-user API keys with caching and env var fallback
pub struct KeyResolver {
    auth_base_url: String,
    service_key: String,
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<(String, String), CachedKey>>>,
    ttl: Duration,
    env_keys: ApiKeys,
}

/// Resolved key for a provider
pub struct ResolvedKey {
    pub api_key: String,
    pub model_override: Option<String>,
    /// Whether this key came from the user's vault (true) or env var fallback (false)
    pub is_user_key: bool,
}

impl KeyResolver {
    /// Create a new key resolver
    pub fn new(
        auth_base_url: String,
        service_key: String,
        env_keys: ApiKeys,
    ) -> Self {
        Self {
            auth_base_url,
            service_key,
            client: reqwest::Client::new(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(300), // 5 min
            env_keys,
        }
    }

    /// Resolve API key for a user + provider
    ///
    /// Priority: per-user key from Gatekeeper > env var default
    pub async fn resolve(
        &self,
        user_id: &str,
        provider: &str,
    ) -> crate::Result<Option<ResolvedKey>> {
        // Check cache first
        let cache_key = (user_id.to_string(), provider.to_string());
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                if cached.expires_at > Instant::now() {
                    return Ok(Some(ResolvedKey {
                        api_key: cached.api_key.clone(),
                        model_override: cached.model_override.clone(),
                        is_user_key: true,
                    }));
                }
            }
        }

        // Fetch from Gatekeeper
        match self.fetch_from_gatekeeper(user_id, provider).await {
            Ok(Some(vault_key)) => {
                // Cache the result
                let mut cache = self.cache.write().await;
                cache.insert(
                    cache_key,
                    CachedKey {
                        api_key: vault_key.api_key.clone(),
                        model_override: vault_key.model_override.clone(),
                        expires_at: Instant::now() + self.ttl,
                    },
                );
                Ok(Some(ResolvedKey {
                    api_key: vault_key.api_key,
                    model_override: vault_key.model_override,
                    is_user_key: true,
                }))
            }
            Ok(None) => {
                // No per-user key, fall back to env var
                self.env_fallback(provider)
            }
            Err(e) => {
                // Gatekeeper unreachable, fall back to env var
                tracing::warn!(error = %e, "gatekeeper unreachable, using env fallback");
                self.env_fallback(provider)
            }
        }
    }

    /// Fetch key from Gatekeeper vault service endpoint
    async fn fetch_from_gatekeeper(
        &self,
        user_id: &str,
        provider: &str,
    ) -> crate::Result<Option<VaultKeyResponse>> {
        let url = format!(
            "{}/api/vault/keys/{}/{}",
            self.auth_base_url, user_id, provider
        );

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.service_key))
            .send()
            .await
            .map_err(|e| crate::Error::Vault(format!("identity service request failed: {e}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(crate::Error::Vault(format!(
                "identity service returned {}",
                response.status()
            )));
        }

        let key: VaultKeyResponse = response
            .json()
            .await
            .map_err(|e| crate::Error::Vault(format!("invalid response: {e}")))?;

        Ok(Some(key))
    }

    /// Fall back to env var key
    fn env_fallback(&self, provider: &str) -> crate::Result<Option<ResolvedKey>> {
        let key = match provider {
            "anthropic" => self.env_keys.anthropic.clone(),
            "openai" => self.env_keys.openai.clone(),
            "openrouter" => self.env_keys.openrouter.clone(),
            _ => None,
        };

        Ok(key.map(|api_key| ResolvedKey {
            api_key,
            model_override: None,
            is_user_key: false,
        }))
    }

    /// Clear cached key for a user+provider
    pub async fn invalidate(&self, user_id: &str, provider: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(&(user_id.to_string(), provider.to_string()));
    }

    /// Store an API key in Gatekeeper vault for a user+provider
    ///
    /// Gatekeeper handles validation and encryption
    pub async fn store(
        &self,
        user_id: &str,
        provider: &str,
        api_key: &str,
        model: Option<&str>,
    ) -> crate::Result<StoreKeyResponse> {
        let url = format!(
            "{}/api/vault/keys/{}/{}",
            self.auth_base_url, user_id, provider
        );

        let body = StoreKeyRequest {
            api_key: api_key.to_string(),
            model: model.map(String::from),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.service_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::Error::Vault(format!("identity service store request failed: {e}")))?;

        let status = response.status();

        if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            // Gatekeeper validated the key and it failed
            let error_body: serde_json::Value = response
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": "Invalid API key"}));

            let message = error_body["error"]
                .as_str()
                .unwrap_or("Invalid API key")
                .to_string();

            return Ok(StoreKeyResponse {
                success: false,
                message,
            });
        }

        if !status.is_success() {
            return Err(crate::Error::Vault(format!(
                "identity service returned {status}"
            )));
        }

        // Invalidate cache so next resolve picks up the new key
        self.invalidate(user_id, provider).await;

        let result: StoreKeyResponse = response
            .json()
            .await
            .map_err(|e| crate::Error::Vault(format!("invalid store response: {e}")))?;

        Ok(result)
    }

    /// Delete an API key from Gatekeeper vault for a user+provider
    pub async fn delete(
        &self,
        user_id: &str,
        provider: &str,
    ) -> crate::Result<DeleteKeyResponse> {
        let url = format!(
            "{}/api/vault/keys/{}/{}",
            self.auth_base_url, user_id, provider
        );

        let response = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.service_key))
            .send()
            .await
            .map_err(|e| crate::Error::Vault(format!("identity service delete request failed: {e}")))?;

        let status = response.status();

        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(DeleteKeyResponse {
                success: false,
                message: "No key configured for this provider".to_string(),
            });
        }

        if !status.is_success() {
            return Err(crate::Error::Vault(format!(
                "identity service returned {status}"
            )));
        }

        // Invalidate cache after deletion
        self.invalidate(user_id, provider).await;

        let result: DeleteKeyResponse = response
            .json()
            .await
            .map_err(|e| crate::Error::Vault(format!("invalid delete response: {e}")))?;

        Ok(result)
    }

    /// Check which providers a user has configured in Gatekeeper
    ///
    /// Probes each supported provider via the existing GET endpoint
    pub async fn list_configured(&self, user_id: &str) -> Vec<ConfiguredProvider> {
        let providers = ["anthropic", "openai", "openrouter"];

        let mut results = Vec::with_capacity(providers.len());

        for provider in &providers {
            let has_key = self
                .fetch_from_gatekeeper(user_id, provider)
                .await
                .ok()
                .flatten()
                .is_some();

            results.push(ConfiguredProvider {
                provider: (*provider).to_string(),
                has_user_key: has_key,
            });
        }

        results
    }
}
