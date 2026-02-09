//! API key authentication middleware

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use super::ApiState;

/// Extract API key from Authorization header
fn extract_api_key(req: &Request) -> Option<&str> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Middleware to verify API key
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

    // Check for valid API key
    let provided_key = extract_api_key(&req);

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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn test_extract_api_key() {
        let mut req = Request::builder().body(Body::empty()).unwrap();

        // No header
        assert_eq!(extract_api_key(&req), None);

        // With Bearer token
        req.headers_mut().insert(
            "authorization",
            HeaderValue::from_static("Bearer test-key-123"),
        );
        assert_eq!(extract_api_key(&req), Some("test-key-123"));
    }
}
