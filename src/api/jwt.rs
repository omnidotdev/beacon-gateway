//! JWT validation for Gatekeeper tokens

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

/// Claims extracted from the JWT
#[derive(Debug, Deserialize)]
pub struct GatekeeperClaims {
    pub sub: String,
    pub exp: u64,
    pub iss: Option<String>,
}

/// Cached JWKS for validating JWTs
pub struct JwksCache {
    auth_base_url: String,
    client: reqwest::Client,
    keys: Arc<RwLock<Option<CachedJwks>>>,
    jwks_uri: Arc<RwLock<Option<String>>>,
}

struct CachedJwks {
    keys: Vec<jsonwebtoken::jwk::Jwk>,
    expires_at: Instant,
}

/// OIDC discovery document (partial)
#[derive(Deserialize)]
struct OidcDiscovery {
    jwks_uri: Option<String>,
}

impl JwksCache {
    pub fn new(auth_base_url: String) -> Self {
        Self {
            auth_base_url,
            client: reqwest::Client::new(),
            keys: Arc::new(RwLock::new(None)),
            jwks_uri: Arc::new(RwLock::new(None)),
        }
    }

    /// Validate a JWT and return the claims
    pub async fn validate(&self, token: &str) -> Result<GatekeeperClaims, String> {
        let jwks = self.get_jwks().await?;

        // Log JWT header for diagnostics
        let header = decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;
        tracing::debug!(
            alg = ?header.alg,
            kid = ?header.kid,
            jwks_count = jwks.len(),
            "validating JWT"
        );

        let mut last_error = None;

        // Try each key until one works (key rotation support)
        for jwk in &jwks {
            let key = match DecodingKey::from_jwk(jwk) {
                Ok(k) => k,
                Err(e) => {
                    tracing::debug!(
                        jwk_kid = ?jwk.common.key_id,
                        jwk_alg = ?jwk.common.key_algorithm,
                        error = %e,
                        "skipping JWK: failed to create decoding key"
                    );
                    continue;
                }
            };

            // jsonwebtoken 9.x requires validation.algorithms to contain ONLY
            // algorithms matching the key's family (it checks every listed alg
            // against the key). Use the JWT header's algorithm directly
            let mut validation = Validation::new(header.alg);
            validation.validate_exp = true;
            // Gateway doesn't have a specific audience to check
            validation.validate_aud = false;
            validation.required_spec_claims.remove("aud");

            match decode::<GatekeeperClaims>(token, &key, &validation) {
                Ok(data) => return Ok(data.claims),
                Err(e) => {
                    tracing::debug!(
                        jwk_kid = ?jwk.common.key_id,
                        error = %e,
                        "JWK did not validate token"
                    );
                    last_error = Some(e);
                    continue;
                }
            }
        }

        Err(format!(
            "no valid key found for JWT (alg={:?}, kid={:?}, jwks_keys={}, last_error={:?})",
            header.alg,
            header.kid,
            jwks.len(),
            last_error,
        ))
    }

    /// Resolve the JWKS URI from OIDC discovery (cached)
    async fn resolve_jwks_uri(&self) -> Result<String, String> {
        // Check cache
        {
            let cached = self.jwks_uri.read().await;
            if let Some(uri) = cached.as_ref() {
                return Ok(uri.clone());
            }
        }

        // Try OIDC discovery first
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.auth_base_url
        );

        let jwks_url = match self.client.get(&discovery_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<OidcDiscovery>().await {
                    Ok(doc) if doc.jwks_uri.is_some() => {
                        let uri = doc.jwks_uri.unwrap();
                        tracing::info!(
                            jwks_uri = %uri,
                            "resolved JWKS URI from OIDC discovery"
                        );
                        uri
                    }
                    _ => {
                        let fallback =
                            format!("{}/.well-known/jwks.json", self.auth_base_url);
                        tracing::debug!(
                            fallback = %fallback,
                            "OIDC discovery missing jwks_uri, using fallback"
                        );
                        fallback
                    }
                }
            }
            _ => {
                let fallback =
                    format!("{}/.well-known/jwks.json", self.auth_base_url);
                tracing::debug!(
                    fallback = %fallback,
                    "OIDC discovery unavailable, using fallback"
                );
                fallback
            }
        };

        let mut cached = self.jwks_uri.write().await;
        *cached = Some(jwks_url.clone());

        Ok(jwks_url)
    }

    /// Fetch JWKS from identity provider (cached for 1 hour)
    async fn get_jwks(&self) -> Result<Vec<jsonwebtoken::jwk::Jwk>, String> {
        // Check cache
        {
            let cache = self.keys.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.keys.clone());
                }
            }
        }

        // Resolve JWKS URI from OIDC discovery
        let url = self.resolve_jwks_uri().await?;

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("failed to fetch JWKS from {url}: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "JWKS endpoint returned {}: {url}",
                response.status()
            ));
        }

        let jwk_set: jsonwebtoken::jwk::JwkSet = response
            .json()
            .await
            .map_err(|e| format!("invalid JWKS response from {url}: {e}"))?;

        tracing::debug!(
            url = %url,
            key_count = jwk_set.keys.len(),
            "fetched JWKS"
        );

        // Cache for 1 hour
        let keys = jwk_set.keys;
        let mut cache = self.keys.write().await;
        *cache = Some(CachedJwks {
            keys: keys.clone(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        });

        Ok(keys)
    }
}
