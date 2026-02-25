//! Retry with exponential backoff for Telegram Bot API calls

use std::time::{Duration, SystemTime};

/// Retry policy for Telegram Bot API calls
///
/// Controls how many times a failed request is retried and how
/// long to wait between attempts using exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Base delay between retries (doubles each attempt)
    pub base_delay: Duration,
    /// Maximum delay cap
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// Determine whether an HTTP status and response body indicate a recoverable error.
///
/// Recoverable errors are worth retrying: rate limits (429), server errors (5xx),
/// and certain transient network-level failures surfaced in the body text.
#[must_use]
pub fn is_recoverable(status: u16, body: &str) -> bool {
    if status == 429 {
        return true;
    }

    if (500..600).contains(&status) {
        return true;
    }

    let lower = body.to_lowercase();
    lower.contains("connection reset")
        || lower.contains("timed out")
        || lower.contains("dns error")
}

/// Extract a `retry_after` duration from a Telegram Bot API error body.
///
/// Telegram encodes the value in seconds at `parameters.retry_after`.
/// Returns `None` if the field is absent or the body is not valid JSON.
///
/// # Examples
///
/// ```ignore
/// use beacon_gateway::channels::telegram::retry::parse_retry_after;
/// let body = r#"{"parameters": {"retry_after": 30}}"#;
/// let dur = parse_retry_after(body);
/// assert_eq!(dur, Some(std::time::Duration::from_secs(30)));
/// ```
#[must_use]
pub fn parse_retry_after(body: &str) -> Option<Duration> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let secs = v.get("parameters")?.get("retry_after")?.as_u64()?;

    Some(Duration::from_secs(secs))
}

/// Compute the delay before the next retry attempt.
///
/// When `retry_after` is provided (e.g. from a 429 response), that value is
/// used directly but capped at `policy.max_delay`. Otherwise the delay follows
/// exponential backoff: `min(base_delay * 2^attempt + jitter, max_delay)`.
///
/// Jitter is 0-25% of the computed delay, derived from `SystemTime` to avoid
/// pulling in a full random number generator.
#[must_use]
pub fn delay_for_attempt(
    policy: &RetryPolicy,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Duration {
    if let Some(ra) = retry_after {
        return ra.min(policy.max_delay);
    }

    let base = policy
        .base_delay
        .saturating_mul(2u32.saturating_pow(attempt));
    let base = base.min(policy.max_delay);

    // Derive a simple jitter from subsecond nanos of the system clock
    let jitter_nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();

    // Scale to 0-25% of the base delay
    let jitter_fraction = (jitter_nanos % 250) as f64 / 1000.0;
    let jitter = base.mul_f64(jitter_fraction);

    (base + jitter).min(policy.max_delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- is_recoverable -------------------------------------------------------

    #[test]
    fn recoverable_on_rate_limit() {
        assert!(is_recoverable(429, ""));
    }

    #[test]
    fn recoverable_on_server_errors() {
        assert!(is_recoverable(500, ""));
        assert!(is_recoverable(502, ""));
        assert!(is_recoverable(503, ""));
        assert!(is_recoverable(599, ""));
    }

    #[test]
    fn not_recoverable_on_client_errors() {
        assert!(!is_recoverable(400, ""));
        assert!(!is_recoverable(401, ""));
        assert!(!is_recoverable(403, ""));
        assert!(!is_recoverable(404, ""));
    }

    #[test]
    fn not_recoverable_on_success() {
        assert!(!is_recoverable(200, ""));
    }

    #[test]
    fn recoverable_on_connection_reset_body() {
        assert!(is_recoverable(200, "Connection Reset by peer"));
    }

    #[test]
    fn recoverable_on_timed_out_body() {
        assert!(is_recoverable(200, "request Timed Out"));
    }

    #[test]
    fn recoverable_on_dns_error_body() {
        assert!(is_recoverable(200, "DNS Error: name not resolved"));
    }

    #[test]
    fn not_recoverable_on_unrelated_body() {
        assert!(!is_recoverable(200, "bad request format"));
    }

    // -- parse_retry_after ----------------------------------------------------

    #[test]
    fn parses_valid_retry_after() {
        let body = r#"{"ok":false,"parameters":{"retry_after":30}}"#;
        assert_eq!(parse_retry_after(body), Some(Duration::from_secs(30)));
    }

    #[test]
    fn parses_retry_after_one_second() {
        let body = r#"{"parameters":{"retry_after":1}}"#;
        assert_eq!(parse_retry_after(body), Some(Duration::from_secs(1)));
    }

    #[test]
    fn returns_none_for_missing_field() {
        let body = r#"{"ok":false,"parameters":{}}"#;
        assert_eq!(parse_retry_after(body), None);
    }

    #[test]
    fn returns_none_for_missing_parameters() {
        let body = r#"{"ok":false}"#;
        assert_eq!(parse_retry_after(body), None);
    }

    #[test]
    fn returns_none_for_invalid_json() {
        assert_eq!(parse_retry_after("not json"), None);
    }

    #[test]
    fn returns_none_for_empty_body() {
        assert_eq!(parse_retry_after(""), None);
    }

    // -- delay_for_attempt ----------------------------------------------------

    #[test]
    fn respects_retry_after() {
        let policy = RetryPolicy::default();
        let ra = Duration::from_secs(10);
        assert_eq!(delay_for_attempt(&policy, 0, Some(ra)), ra);
    }

    #[test]
    fn caps_retry_after_at_max_delay() {
        let policy = RetryPolicy {
            max_delay: Duration::from_secs(5),
            ..RetryPolicy::default()
        };
        let ra = Duration::from_secs(60);
        assert_eq!(delay_for_attempt(&policy, 0, Some(ra)), policy.max_delay);
    }

    #[test]
    fn exponential_growth() {
        let policy = RetryPolicy {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(60),
            ..RetryPolicy::default()
        };

        let d0 = delay_for_attempt(&policy, 0, None);
        let d1 = delay_for_attempt(&policy, 1, None);
        let d2 = delay_for_attempt(&policy, 2, None);

        // Each attempt's base doubles; jitter adds up to 25%, so the lower
        // bound of the next attempt should exceed the previous base
        assert!(d0 >= Duration::from_millis(100), "attempt 0: {d0:?}");
        assert!(d1 >= Duration::from_millis(200), "attempt 1: {d1:?}");
        assert!(d2 >= Duration::from_millis(400), "attempt 2: {d2:?}");
    }

    #[test]
    fn delay_capped_at_max() {
        let policy = RetryPolicy {
            base_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(15),
            ..RetryPolicy::default()
        };

        // 10s * 2^3 = 80s, should be capped at 15s
        let d = delay_for_attempt(&policy, 3, None);
        assert!(d <= policy.max_delay, "delay {d:?} exceeds max");
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let policy = RetryPolicy {
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(60),
            ..RetryPolicy::default()
        };

        // Run multiple times; jitter should keep delay within [base, base * 1.25]
        for _ in 0..50 {
            let d = delay_for_attempt(&policy, 0, None);
            assert!(d >= Duration::from_millis(1000), "below base: {d:?}");
            assert!(d <= Duration::from_millis(1250), "above 125%: {d:?}");
        }
    }

    // -- Default policy -------------------------------------------------------

    #[test]
    fn default_policy_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay, Duration::from_millis(500));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
    }
}
