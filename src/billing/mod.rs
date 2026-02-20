//! Aether billing integration for Beacon gateway
//!
//! Provides subscription entitlement and usage-limit enforcement via
//! the Aether billing service. Enabled when `AETHER_URL` is set.

pub mod middleware;

use std::sync::Arc;
use std::time::Duration;

use mini_moka::sync::Cache;
use secrecy::SecretString;
use synapse_billing::AetherClient;

/// Fail mode used when Aether is unreachable
#[derive(Clone, Debug)]
pub enum FailMode {
    /// Allow the request through and log a warning
    Open,
    /// Reject the request with 503
    Closed,
}

/// Shared billing state passed to the middleware
#[derive(Clone)]
pub struct BillingState {
    /// Aether API client
    pub client: Arc<AetherClient>,
    /// Fail mode for Aether errors
    pub fail_mode: FailMode,
    /// TTL cache for entitlement and usage results
    pub cache: BillingCache,
}

impl BillingState {
    /// Initialize from environment variables.
    ///
    /// Returns `None` if `AETHER_URL` is not set (billing disabled).
    ///
    /// # Errors
    ///
    /// Returns an error if `AETHER_URL` is not a valid URL, if
    /// `AETHER_SERVICE_API_KEY` is missing when `AETHER_URL` is set, or
    /// if the `AetherClient` cannot be constructed.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(aether_url) = std::env::var("AETHER_URL").ok() else {
            return Ok(None);
        };

        let aether_url: url::Url = aether_url.parse()?;

        let app_id = std::env::var("AETHER_APP_ID").unwrap_or_else(|_| "synapse".to_string());

        let api_key_str = std::env::var("AETHER_SERVICE_API_KEY")
            .map_err(|_| anyhow::anyhow!("AETHER_SERVICE_API_KEY is required when AETHER_URL is set"))?;
        let service_api_key = SecretString::new(api_key_str.into());

        let fail_mode = match std::env::var("AETHER_FAIL_MODE").as_deref() {
            Ok("closed") => FailMode::Closed,
            _ => FailMode::Open,
        };

        let cache_ttl_secs: u64 = std::env::var("AETHER_CACHE_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);

        let client = AetherClient::new(aether_url, app_id, service_api_key)?;

        tracing::info!("Aether billing enabled");

        Ok(Some(Self {
            client: Arc::new(client),
            fail_mode,
            cache: BillingCache::new(cache_ttl_secs),
        }))
    }
}

/// Cache key for entitlement checks
#[derive(Hash, Eq, PartialEq, Clone)]
struct EntitlementKey {
    entity_type: String,
    entity_id: String,
    feature_key: String,
}

/// Cache key for usage checks
#[derive(Hash, Eq, PartialEq, Clone)]
struct UsageKey {
    entity_type: String,
    entity_id: String,
    meter_key: String,
}

/// Cached entitlement result
#[derive(Clone)]
pub struct CachedEntitlement {
    pub has_access: bool,
}

/// Cached usage check result
#[derive(Clone)]
pub struct CachedUsage {
    pub allowed: bool,
}

/// TTL-based cache for entitlement and usage check results
#[derive(Clone)]
pub struct BillingCache {
    entitlements: Cache<EntitlementKey, CachedEntitlement>,
    usage: Cache<UsageKey, CachedUsage>,
}

impl BillingCache {
    /// Create a new cache with the given TTL in seconds
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        let ttl = Duration::from_secs(ttl_secs);
        Self {
            entitlements: Cache::builder()
                .max_capacity(1024)
                .time_to_live(ttl)
                .build(),
            usage: Cache::builder()
                .max_capacity(1024)
                .time_to_live(ttl)
                .build(),
        }
    }

    /// Look up a cached entitlement result
    #[must_use]
    pub fn get_entitlement(
        &self,
        entity_type: &str,
        entity_id: &str,
        feature_key: &str,
    ) -> Option<CachedEntitlement> {
        self.entitlements.get(&EntitlementKey {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            feature_key: feature_key.to_string(),
        })
    }

    /// Store an entitlement result in the cache
    pub fn put_entitlement(
        &self,
        entity_type: &str,
        entity_id: &str,
        feature_key: &str,
        value: CachedEntitlement,
    ) {
        self.entitlements.insert(
            EntitlementKey {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
                feature_key: feature_key.to_string(),
            },
            value,
        );
    }

    /// Look up a cached usage check result
    #[must_use]
    pub fn get_usage(
        &self,
        entity_type: &str,
        entity_id: &str,
        meter_key: &str,
    ) -> Option<CachedUsage> {
        self.usage.get(&UsageKey {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            meter_key: meter_key.to_string(),
        })
    }

    /// Store a usage check result in the cache
    pub fn put_usage(
        &self,
        entity_type: &str,
        entity_id: &str,
        meter_key: &str,
        value: CachedUsage,
    ) {
        self.usage.insert(
            UsageKey {
                entity_type: entity_type.to_string(),
                entity_id: entity_id.to_string(),
                meter_key: meter_key.to_string(),
            },
            value,
        );
    }
}
