//! Rate limiting for cloud mode

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::InMemoryState, state::NotKeyed};

use super::ApiState;

/// Global rate limiter
pub type SharedLimiter = Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>;

/// Create a rate limiter with the given requests-per-minute burst capacity
#[must_use]
pub fn create_limiter(requests_per_minute: u32) -> SharedLimiter {
    let rpm = NonZeroU32::new(requests_per_minute).unwrap_or(NonZeroU32::MIN);
    let quota = Quota::per_minute(rpm);
    Arc::new(RateLimiter::direct(quota))
}

/// Rate limiting middleware (only active when limiter is configured)
///
/// # Errors
///
/// Returns `TOO_MANY_REQUESTS` when the rate limit is exceeded
pub async fn rate_limit_middleware(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(ref limiter) = state.rate_limiter
        && limiter.check().is_err()
    {
        tracing::warn!("rate limit exceeded");
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(next.run(req).await)
}
