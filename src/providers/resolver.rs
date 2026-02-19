//! Resolve per-user provider keys from Synapse

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::ApiKeys;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SynapseProviderKey {
    provider: String,
    decrypted_key: String,
    model_preference: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SynapseProviderKeysResponse {
    provider_keys: Vec<SynapseProviderKey>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveProviderKeysRequest {
    identity_provider_id: String,
}

#[derive(Debug)]
struct CachedUserKeys {
    keys: HashMap<String, ResolvedKey>,
    expires_at: Instant,
}

/// Resolved key for a provider
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    pub api_key: String,
    pub model_override: Option<String>,
    /// Whether this key came from the user's vault (true) or env var fallback (false)
    pub is_user_key: bool,
}

/// Resolve per-user API keys with caching and env var fallback
pub struct KeyResolver {
    synapse_api_url: String,
    gateway_secret: String,
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<String, CachedUserKeys>>>,
    ttl: Duration,
    env_keys: ApiKeys,
}

impl KeyResolver {
    /// Create a new key resolver
    pub fn new(synapse_api_url: String, gateway_secret: String, env_keys: ApiKeys) -> Self {
        Self {
            synapse_api_url,
            gateway_secret,
            client: reqwest::Client::new(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(300), // 5 min
            env_keys,
        }
    }

    /// Resolve API key for a user + provider
    ///
    /// Fetches all keys for the user from Synapse in one call, caches the full set,
    /// returns the one for the requested provider. Falls back to env vars if Synapse
    /// is unreachable.
    pub async fn resolve(
        &self,
        identity_provider_id: &str,
        provider: &str,
    ) -> crate::Result<Option<ResolvedKey>> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(identity_provider_id) {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.keys.get(provider).cloned()
                        .or_else(|| self.env_fallback(provider).unwrap_or(None)));
                }
            }
        }

        // Fetch all keys for this user from Synapse
        match self.fetch_from_synapse(identity_provider_id).await {
            Ok(synapse_keys) => {
                let mut keys_map: HashMap<String, ResolvedKey> = HashMap::new();
                for k in &synapse_keys {
                    keys_map.insert(k.provider.clone(), ResolvedKey {
                        api_key: k.decrypted_key.clone(),
                        model_override: k.model_preference.clone(),
                        is_user_key: true,
                    });
                }
                let result = keys_map.get(provider).cloned();
                let mut cache = self.cache.write().await;
                cache.insert(identity_provider_id.to_string(), CachedUserKeys {
                    keys: keys_map,
                    expires_at: Instant::now() + self.ttl,
                });
                Ok(result.or_else(|| self.env_fallback(provider).unwrap_or(None)))
            }
            Err(e) => {
                tracing::warn!(error = %e, "synapse unreachable, using env fallback");
                self.env_fallback(provider)
            }
        }
    }

    /// Fetch all provider keys for a user from Synapse in one request
    async fn fetch_from_synapse(
        &self,
        identity_provider_id: &str,
    ) -> crate::Result<Vec<SynapseProviderKey>> {
        let url = format!(
            "{}/internal/resolve-provider-keys",
            self.synapse_api_url.trim_end_matches('/')
        );
        let response = self.client
            .post(&url)
            .header("x-gateway-secret", &self.gateway_secret)
            .json(&ResolveProviderKeysRequest {
                identity_provider_id: identity_provider_id.to_string(),
            })
            .send()
            .await
            .map_err(|e| crate::Error::Vault(format!("synapse request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(crate::Error::Vault(format!(
                "synapse returned {}",
                response.status()
            )));
        }
        let body: SynapseProviderKeysResponse = response
            .json()
            .await
            .map_err(|e| crate::Error::Vault(format!("invalid synapse response: {e}")))?;
        Ok(body.provider_keys)
    }

    /// Fall back to env var key when Synapse is unreachable or user has no configured keys
    fn env_fallback(&self, provider: &str) -> crate::Result<Option<ResolvedKey>> {
        let key = match provider {
            "anthropic" => self.env_keys.anthropic.clone(),
            "openai" => self.env_keys.openai.clone(),
            "openrouter" => self.env_keys.openrouter.clone(),
            _ => None,
        };
        Ok(key.map(|api_key| ResolvedKey { api_key, model_override: None, is_user_key: false }))
    }

    /// Clear the cached keys for a user
    pub async fn invalidate(&self, identity_provider_id: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(identity_provider_id);
    }

    /// Return the list of configured provider names for a user
    pub async fn list_configured(&self, identity_provider_id: &str) -> Vec<String> {
        // Try cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(identity_provider_id) {
                if cached.expires_at > Instant::now() {
                    return cached.keys.keys().cloned().collect();
                }
            }
        }
        // Fetch from Synapse
        match self.fetch_from_synapse(identity_provider_id).await {
            Ok(keys) => {
                let providers: Vec<String> = keys.iter().map(|k| k.provider.clone()).collect();
                let mut keys_map = HashMap::new();
                for k in keys {
                    keys_map.insert(k.provider.clone(), ResolvedKey {
                        api_key: k.decrypted_key,
                        model_override: k.model_preference,
                        is_user_key: true,
                    });
                }
                let mut cache = self.cache.write().await;
                cache.insert(identity_provider_id.to_string(), CachedUserKeys {
                    keys: keys_map,
                    expires_at: Instant::now() + self.ttl,
                });
                providers
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch configured providers from synapse");
                vec![]
            }
        }
    }
}
