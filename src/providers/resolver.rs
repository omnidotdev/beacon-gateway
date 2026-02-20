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
    default_provider: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveProviderKeysRequest {
    identity_provider_id: String,
}

#[derive(Debug)]
struct CachedUserKeys {
    keys: HashMap<String, ResolvedKey>,
    default_provider: Option<String>,
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
    #[must_use]
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
    ///
    /// # Errors
    ///
    /// Returns an error if Synapse is unreachable and the env fallback also fails.
    pub async fn resolve(
        &self,
        identity_provider_id: &str,
        provider: &str,
    ) -> crate::Result<Option<ResolvedKey>> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(identity_provider_id)
                && cached.expires_at > Instant::now()
            {
                return Ok(cached.keys.get(provider).cloned()
                    .or_else(|| self.env_fallback(provider)));
            }
        }

        // Fetch all keys for this user from Synapse
        match self.fetch_from_synapse(identity_provider_id).await {
            Ok(resp) => {
                let mut keys_map: HashMap<String, ResolvedKey> = HashMap::new();
                for k in &resp.provider_keys {
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
                    default_provider: resp.default_provider,
                    expires_at: Instant::now() + self.ttl,
                });
                Ok(result.or_else(|| self.env_fallback(provider)))
            }
            Err(e) => {
                tracing::warn!(error = %e, "synapse unreachable, using env fallback");
                Ok(self.env_fallback(provider))
            }
        }
    }

    /// Fetch all provider keys for a user from Synapse in one request
    async fn fetch_from_synapse(
        &self,
        identity_provider_id: &str,
    ) -> crate::Result<SynapseProviderKeysResponse> {
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
        Ok(body)
    }

    /// Fall back to env var key when Synapse is unreachable or user has no configured keys
    fn env_fallback(&self, provider: &str) -> Option<ResolvedKey> {
        let key = match provider {
            "anthropic" => self.env_keys.anthropic.clone(),
            "openai" => self.env_keys.openai.clone(),
            "openrouter" => self.env_keys.openrouter.clone(),
            _ => None,
        };
        key.map(|api_key| ResolvedKey { api_key, model_override: None, is_user_key: false })
    }

    /// Clear the cached keys for a user
    pub async fn invalidate(&self, identity_provider_id: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(identity_provider_id);
    }

    /// Select the best available provider key from a cached entry.
    /// Respects `default_provider` if set and has a user key; otherwise falls
    /// back to priority order: `anthropic` → `openai` → `openrouter` → `omni_credits`.
    fn preferred_from_cache(cached: &CachedUserKeys) -> Option<(String, ResolvedKey)> {
        // Respect the user's explicit preference first
        if let Some(ref default) = cached.default_provider
            && let Some(key) = cached.keys.get(default)
            && key.is_user_key
        {
            return Some((default.clone(), key.clone()));
        }

        // Fall back to priority order
        for provider in &["anthropic", "openai", "openrouter", "omni_credits"] {
            if let Some(key) = cached.keys.get(*provider).filter(|k| k.is_user_key) {
                return Some((provider.to_string(), key.clone()));
            }
        }

        None
    }

    /// Resolve the user's preferred provider key.
    ///
    /// Checks the user's `defaultProvider` Synapse preference first, then falls
    /// back to the priority order. Returns `None` if the user has no configured keys.
    /// Falls back to env vars if Synapse is unreachable.
    ///
    /// # Errors
    ///
    /// Returns an error if Synapse is unreachable and no env fallback keys are configured.
    pub async fn resolve_preferred(
        &self,
        identity_provider_id: &str,
    ) -> crate::Result<Option<(String, ResolvedKey)>> {
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(identity_provider_id)
                && cached.expires_at > Instant::now()
            {
                return Ok(Self::preferred_from_cache(cached));
            }
        }

        // Cache miss — fetch fresh
        match self.fetch_from_synapse(identity_provider_id).await {
            Ok(resp) => {
                let mut keys_map: HashMap<String, ResolvedKey> = HashMap::new();
                for k in &resp.provider_keys {
                    keys_map.insert(k.provider.clone(), ResolvedKey {
                        api_key: k.decrypted_key.clone(),
                        model_override: k.model_preference.clone(),
                        is_user_key: true,
                    });
                }
                let cached = CachedUserKeys {
                    keys: keys_map,
                    default_provider: resp.default_provider,
                    expires_at: Instant::now() + self.ttl,
                };
                let result = Self::preferred_from_cache(&cached);
                {
                    let mut cache = self.cache.write().await;
                    cache.insert(identity_provider_id.to_string(), cached);
                }
                Ok(result)
            }
            Err(e) => {
                tracing::warn!(error = %e, "synapse unreachable in resolve_preferred, using env fallback");
                Ok(self.env_preferred())
            }
        }
    }

    /// Fall back to env var keys when Synapse is unreachable, using priority order
    fn env_preferred(&self) -> Option<(String, ResolvedKey)> {
        for (provider, key_opt) in &[
            ("anthropic", &self.env_keys.anthropic),
            ("openai", &self.env_keys.openai),
            ("openrouter", &self.env_keys.openrouter),
        ] {
            if let Some(key) = key_opt {
                return Some((provider.to_string(), ResolvedKey {
                    api_key: key.clone(),
                    model_override: None,
                    is_user_key: false,
                }));
            }
        }
        None
    }

    /// Return the list of configured provider names for a user
    pub async fn list_configured(&self, identity_provider_id: &str) -> Vec<String> {
        // Try cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(identity_provider_id)
                && cached.expires_at > Instant::now()
            {
                return cached.keys.keys().cloned().collect();
            }
        }
        // Fetch from Synapse
        match self.fetch_from_synapse(identity_provider_id).await {
            Ok(resp) => {
                let providers: Vec<String> = resp.provider_keys.iter().map(|k| k.provider.clone()).collect();
                let mut keys_map = HashMap::new();
                for k in resp.provider_keys {
                    keys_map.insert(k.provider.clone(), ResolvedKey {
                        api_key: k.decrypted_key,
                        model_override: k.model_preference,
                        is_user_key: true,
                    });
                }
                let mut cache = self.cache.write().await;
                cache.insert(identity_provider_id.to_string(), CachedUserKeys {
                    keys: keys_map,
                    default_provider: resp.default_provider,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_resolver() -> KeyResolver {
        KeyResolver {
            synapse_api_url: "http://test".to_string(),
            gateway_secret: "secret".to_string(),
            client: reqwest::Client::new(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(300),
            env_keys: ApiKeys::default(),
        }
    }

    #[test]
    fn preferred_from_cache_uses_default_provider() {
        let mut keys = HashMap::new();
        keys.insert("anthropic".to_string(), ResolvedKey {
            api_key: "sk-ant".to_string(),
            model_override: None,
            is_user_key: true,
        });
        keys.insert("openai".to_string(), ResolvedKey {
            api_key: "sk-openai".to_string(),
            model_override: None,
            is_user_key: true,
        });

        let cached = CachedUserKeys {
            keys,
            default_provider: Some("openai".to_string()),
            expires_at: Instant::now() + Duration::from_secs(300),
        };

        let _resolver = make_resolver();
        let result = KeyResolver::preferred_from_cache(&cached);
        assert!(result.is_some());
        let (provider, _key) = result.unwrap();
        assert_eq!(provider, "openai"); // defaultProvider wins over anthropic
    }

    #[test]
    fn preferred_from_cache_falls_back_to_priority_when_no_default() {
        let mut keys = HashMap::new();
        keys.insert("openai".to_string(), ResolvedKey {
            api_key: "sk-openai".to_string(),
            model_override: None,
            is_user_key: true,
        });

        let cached = CachedUserKeys {
            keys,
            default_provider: None,
            expires_at: Instant::now() + Duration::from_secs(300),
        };

        let result = KeyResolver::preferred_from_cache(&cached);
        assert!(result.is_some());
        let (provider, _key) = result.unwrap();
        assert_eq!(provider, "openai");
    }

    #[test]
    fn preferred_from_cache_returns_none_when_empty() {
        let cached = CachedUserKeys {
            keys: HashMap::new(),
            default_provider: None,
            expires_at: Instant::now() + Duration::from_secs(300),
        };

        let result = KeyResolver::preferred_from_cache(&cached);
        assert!(result.is_none());
    }

    #[test]
    fn preferred_from_cache_skips_non_user_key_for_default() {
        // If default_provider points to an env fallback key, it should be skipped
        // and fall through to the priority list
        let mut keys = HashMap::new();
        keys.insert("anthropic".to_string(), ResolvedKey {
            api_key: "sk-ant-env".to_string(),
            model_override: None,
            is_user_key: false, // env fallback, not user key
        });
        keys.insert("openai".to_string(), ResolvedKey {
            api_key: "sk-openai-user".to_string(),
            model_override: None,
            is_user_key: true,
        });

        let cached = CachedUserKeys {
            keys,
            default_provider: Some("anthropic".to_string()), // default is anthropic but it's env key
            expires_at: Instant::now() + Duration::from_secs(300),
        };

        let result = KeyResolver::preferred_from_cache(&cached);
        assert!(result.is_some());
        let (provider, _key) = result.unwrap();
        // anthropic is skipped (not user key), falls to openai
        assert_eq!(provider, "openai");
    }
}
