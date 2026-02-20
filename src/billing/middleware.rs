//! Aether entitlement middleware
//!
//! Enforces subscription entitlements and usage limits before processing
//! AI requests. Skips checks for unauthenticated or non-JWT requests.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{BillingState, CachedEntitlement, CachedUsage, FailMode};
use crate::api::jwt::JwksCache;

const FEATURE_KEY_API_ACCESS: &str = "api_access";
const METER_KEY_REQUESTS: &str = "requests";

/// Extract a Bearer token from the Authorization header
fn extract_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from)
}

/// Enforce Aether entitlements and usage limits.
///
/// 1. Extracts and validates the Bearer JWT.
/// 2. Checks `api_access` entitlement → 403 if denied.
/// 3. Checks usage limit for `requests` meter → 429 if exceeded.
/// 4. Passes through if all checks pass.
///
/// Skips checks when:
/// - No Authorization header is present (unauthenticated route)
/// - JWT validation fails (other middleware will reject the request)
/// - Aether is unreachable and fail mode is open
pub async fn billing_middleware(
    billing: BillingState,
    jwt_cache: Arc<JwksCache>,
    request: Request,
    next: Next,
) -> Response {
    // Extract bearer token — skip if absent (public or non-JWT path)
    let Some(token) = extract_bearer(&request) else {
        return next.run(request).await;
    };

    // Validate JWT and extract user ID from sub claim
    let sub = match jwt_cache.validate(&token).await {
        Ok(claims) => claims.sub,
        Err(e) => {
            tracing::debug!(error = %e, "JWT validation failed, skipping billing checks");
            return next.run(request).await;
        }
    };

    let entity_type = "user";
    let entity_id = sub.as_str();

    // Check api_access entitlement → 403 if denied
    match check_entitlement(&billing, entity_type, entity_id).await {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::FORBIDDEN, "API access not granted for this account")
                .into_response();
        }
        Err(e) => {
            return handle_aether_error(&billing.fail_mode, e, request, next).await;
        }
    }

    // Check requests usage limit → 429 if exceeded
    match check_usage(&billing, entity_type, entity_id).await {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::TOO_MANY_REQUESTS, "usage limit exceeded").into_response();
        }
        Err(e) => {
            return handle_aether_error(&billing.fail_mode, e, request, next).await;
        }
    }

    next.run(request).await
}

/// Check the `api_access` entitlement, using cache when available
async fn check_entitlement(
    state: &BillingState,
    entity_type: &str,
    entity_id: &str,
) -> Result<bool, synapse_billing::BillingError> {
    // Check cache first
    if let Some(cached) = state.cache.get_entitlement(entity_type, entity_id, FEATURE_KEY_API_ACCESS)
    {
        return Ok(cached.has_access);
    }

    // Cache miss — call Aether
    let response = state
        .client
        .check_entitlement(entity_type, entity_id, FEATURE_KEY_API_ACCESS)
        .await?;

    state.cache.put_entitlement(
        entity_type,
        entity_id,
        FEATURE_KEY_API_ACCESS,
        CachedEntitlement {
            has_access: response.has_access,
        },
    );

    Ok(response.has_access)
}

/// Check the `requests` usage meter, using cache when available
async fn check_usage(
    state: &BillingState,
    entity_type: &str,
    entity_id: &str,
) -> Result<bool, synapse_billing::BillingError> {
    // Check cache first
    if let Some(cached) = state.cache.get_usage(entity_type, entity_id, METER_KEY_REQUESTS) {
        return Ok(cached.allowed);
    }

    // Cache miss — call Aether
    let response = state
        .client
        .check_usage(entity_type, entity_id, METER_KEY_REQUESTS, 1.0)
        .await?;

    state.cache.put_usage(
        entity_type,
        entity_id,
        METER_KEY_REQUESTS,
        CachedUsage {
            allowed: response.allowed,
        },
    );

    Ok(response.allowed)
}

/// Handle an Aether communication error according to the configured fail mode
async fn handle_aether_error(
    fail_mode: &FailMode,
    error: synapse_billing::BillingError,
    request: Request,
    next: Next,
) -> Response {
    match fail_mode {
        FailMode::Open => {
            tracing::warn!(
                error = %error,
                "Aether unreachable, allowing request through (fail-open mode)"
            );
            next.run(request).await
        }
        FailMode::Closed => {
            tracing::error!(
                error = %error,
                "Aether unreachable, rejecting request (fail-closed mode)"
            );
            (StatusCode::SERVICE_UNAVAILABLE, "billing service unavailable").into_response()
        }
    }
}
