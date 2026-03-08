//! Authentication middleware
//!
//! Supports two authentication methods:
//! 1. API key (simple Bearer token matching `BEACON_API_KEY`)
//! 2. Gatekeeper JWT (validated via JWKS endpoint)
//!
//! In development mode (no API key configured, no JWKS), all requests pass through

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use super::ApiState;

/// Authenticated user identity extracted from the request
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthIdentity {
    /// User ID (from JWT `sub` claim or "api-key" for API key auth)
    pub user_id: String,
    /// Authentication method used
    pub method: AuthMethod,
}

/// How the user was authenticated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AuthMethod {
    /// Matched the configured `BEACON_API_KEY`
    ApiKey,
    /// Validated via Gatekeeper JWT
    Jwt,
    /// No auth configured (development mode)
    Anonymous,
}

/// Extract Bearer token from Authorization header
fn extract_bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Middleware to verify API key (admin endpoints)
pub async fn require_api_key(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // If no API key configured, allow all requests (development mode)
    let Some(expected_key) = &state.api_key else {
        tracing::warn!("API key not configured - allowing unauthenticated access");
        return Ok(next.run(req).await);
    };

    let provided_key = extract_bearer(&req);

    match provided_key {
        Some(key) if key == expected_key => Ok(next.run(req).await),
        Some(_) => {
            tracing::warn!("invalid API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::debug!("no API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Middleware that accepts either API key or Gatekeeper JWT
///
/// On success, inserts `AuthIdentity` into request extensions.
/// In development mode (no auth configured), passes through as anonymous.
#[allow(dead_code)]
pub async fn require_auth(
    State(state): State<Arc<ApiState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer(&req);

    // Try API key first
    if let Some(expected_key) = &state.api_key
        && let Some(key) = token
        && key == expected_key
    {
        req.extensions_mut().insert(AuthIdentity {
            user_id: "api-key".to_string(),
            method: AuthMethod::ApiKey,
        });
        return Ok(next.run(req).await);
    }

    // Try JWT validation via HIDRA Gatekeeper
    if let Some(ref jwt_cache) = state.jwt_cache
        && let Some(token) = token
    {
        match jwt_cache.validate(token).await {
            Ok(claims) => {
                tracing::debug!(user_id = %claims.sub, "authenticated via Gatekeeper JWT");
                req.extensions_mut().insert(AuthIdentity {
                    user_id: claims.sub,
                    method: AuthMethod::Jwt,
                });
                return Ok(next.run(req).await);
            }
            Err(e) => {
                tracing::debug!(error = %e, "JWT validation failed");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    // Development mode: no API key and no JWKS configured
    if state.api_key.is_none() && state.jwt_cache.is_none() {
        req.extensions_mut().insert(AuthIdentity {
            user_id: "anonymous".to_string(),
            method: AuthMethod::Anonymous,
        });
        return Ok(next.run(req).await);
    }

    // Auth is configured but no valid credentials provided
    tracing::debug!("no valid credentials provided");
    Err(StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn extracts_bearer_token() {
        let mut req = Request::builder().body(Body::empty()).unwrap();

        // No header
        assert_eq!(extract_bearer(&req), None);

        // With Bearer token
        req.headers_mut().insert(
            "authorization",
            HeaderValue::from_static("Bearer test-key-123"),
        );
        assert_eq!(extract_bearer(&req), Some("test-key-123"));
    }

    #[test]
    fn auth_identity_methods() {
        let api_key = AuthIdentity {
            user_id: "api-key".to_string(),
            method: AuthMethod::ApiKey,
        };
        assert_eq!(api_key.method, AuthMethod::ApiKey);

        let jwt = AuthIdentity {
            user_id: "user-123".to_string(),
            method: AuthMethod::Jwt,
        };
        assert_eq!(jwt.method, AuthMethod::Jwt);
    }
}
