//! `OmniEvent` publishing via Iggy HTTP REST API
//!
//! Publishes lifecycle events to the `omni-events` Iggy stream.
//! Publishing is best-effort — errors are logged and never propagate to callers.
//!
//! Initialize once at startup with [`init_publisher`], then call [`publish`] anywhere.

use std::sync::OnceLock;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Iggy HTTP REST API default port
const DEFAULT_HTTP_PORT: u16 = 3000;

/// Iggy stream name for all `OmniEvent`s
const STREAM_NAME: &str = "omni-events";

/// Number of partitions per organization topic
const TOPIC_PARTITIONS: u32 = 3;

/// 90-day message retention in seconds
const RETENTION_SECS: u64 = 90 * 24 * 60 * 60;

/// Global publisher configuration
static CONFIG: OnceLock<EventsConfig> = OnceLock::new();

/// Cached Iggy bearer token — shared across all publish calls
static TOKEN_CACHE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn token_cache() -> &'static RwLock<Option<String>> {
    TOKEN_CACHE.get_or_init(|| RwLock::new(None))
}

/// Retrieve the cached token, authenticating if the cache is empty.
async fn cached_token(
    client: &reqwest::Client,
    config: &EventsConfig,
) -> anyhow::Result<String> {
    // Fast path: read-lock
    {
        let r = token_cache().read().await;
        if let Some(t) = r.as_ref() {
            return Ok(t.clone());
        }
    }
    // Slow path: acquire write-lock and authenticate
    let mut w = token_cache().write().await;
    // Re-check after acquiring write lock (another task may have authenticated)
    if let Some(t) = w.as_ref() {
        return Ok(t.clone());
    }
    let t = login(client, config).await?;
    *w = Some(t.clone());
    Ok(t)
}

/// Invalidate the cached token so the next publish re-authenticates.
async fn invalidate_token() {
    *token_cache().write().await = None;
}

/// Authenticate against the Iggy HTTP API and return a bearer token.
async fn login(client: &reqwest::Client, config: &EventsConfig) -> anyhow::Result<String> {
    let resp: LoginResponse = client
        .post(format!("{}/users/login", config.base_url))
        .json(&LoginRequest {
            username: &config.username,
            password: &config.password,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp.tokens.access.token)
}

/// Configuration for the Iggy HTTP publisher
#[derive(Debug, Clone)]
pub struct EventsConfig {
    /// Iggy HTTP base URL (e.g., `http://localhost:3000`)
    base_url: String,
    /// Iggy username
    username: String,
    /// Iggy password
    password: String,
}

impl EventsConfig {
    /// Load configuration from environment variables.
    ///
    /// Reads `IGGY_HOST` (default: `localhost`), `IGGY_HTTP_PORT` (default: `3000`),
    /// `IGGY_USERNAME` (default: `iggy`), and `IGGY_PASSWORD` (default: `iggy`).
    #[must_use]
    pub fn from_env() -> Self {
        let host = std::env::var("IGGY_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("IGGY_HTTP_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(DEFAULT_HTTP_PORT);
        Self {
            base_url: format!("http://{host}:{port}"),
            username: std::env::var("IGGY_USERNAME").unwrap_or_else(|_| "iggy".to_string()),
            password: std::env::var("IGGY_PASSWORD").unwrap_or_else(|_| "iggy".to_string()),
        }
    }
}

/// An `OmniEvent` published to the event stream
///
/// Matches the standard `OmniEvent` schema used across the Omni platform.
#[derive(Debug, Clone, Serialize)]
pub struct OmniEvent {
    /// Unique event ID (UUID v4)
    pub id: String,
    /// Event type (e.g., `"beacon.message.received"`)
    #[serde(rename = "type")]
    pub event_type: String,
    /// Optional subject for partition routing (e.g., user ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Source service identifier
    pub source: String,
    /// Arbitrary event payload
    pub data: serde_json::Value,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Organization ID — used as the Iggy topic name
    pub organization_id: String,
}

impl OmniEvent {
    /// Create a new event with auto-generated `id` and `timestamp`.
    #[must_use]
    pub fn new(event_type: &str, organization_id: &str, data: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event_type: event_type.to_string(),
            subject: None,
            source: "beacon-gateway".to_string(),
            data,
            timestamp: chrono::Utc::now().to_rfc3339(),
            organization_id: organization_id.to_string(),
        }
    }

    /// Return this event with the given subject set.
    #[must_use]
    pub fn with_subject(mut self, subject: &str) -> Self {
        self.subject = Some(subject.to_string());
        self
    }
}

/// Build a `beacon.conversation.started` event.
///
/// # Arguments
///
/// - `session_id` - Unique session identifier (used as conversation ID and subject)
/// - `channel` - Channel name (e.g., `"discord"`, `"slack"`)
/// - `organization_id` - Organization/user scoping identifier
#[must_use]
pub fn build_conversation_started_event(
    session_id: &str,
    channel: &str,
    organization_id: &str,
) -> OmniEvent {
    OmniEvent::new(
        "beacon.conversation.started",
        organization_id,
        serde_json::json!({
            "conversationId": session_id,
            "channel": channel,
        }),
    )
    .with_subject(session_id)
}

/// Build a `beacon.conversation.ended` event.
///
/// # Arguments
///
/// - `session_id` - Unique session identifier (used as conversation ID and subject)
/// - `channel` - Channel name (e.g., `"discord"`, `"slack"`)
/// - `organization_id` - Organization/user scoping identifier
#[must_use]
pub fn build_conversation_ended_event(
    session_id: &str,
    channel: &str,
    organization_id: &str,
) -> OmniEvent {
    OmniEvent::new(
        "beacon.conversation.ended",
        organization_id,
        serde_json::json!({
            "conversationId": session_id,
            "channel": channel,
        }),
    )
    .with_subject(session_id)
}

/// Build a `beacon.tool.executed` event.
///
/// # Arguments
///
/// - `session_id` - Session identifier for the conversation in which the tool ran
/// - `tool_name` - Name of the tool that was executed
/// - `success` - Whether the tool execution succeeded
/// - `organization_id` - Organization/user scoping identifier
#[must_use]
pub fn build_tool_executed_event(
    session_id: &str,
    tool_name: &str,
    success: bool,
    organization_id: &str,
) -> OmniEvent {
    OmniEvent::new(
        "beacon.tool.executed",
        organization_id,
        serde_json::json!({
            "conversationId": session_id,
            "toolName": tool_name,
            "success": success,
        }),
    )
    .with_subject(session_id)
}

/// Initialize the global Iggy publisher.
///
/// No-op if already initialized. Call once at daemon startup.
pub fn init_publisher(config: EventsConfig) {
    if CONFIG.set(config).is_ok() {
        tracing::info!("Iggy event publisher initialized");
    }
}

/// Publish an `OmniEvent` to Iggy (best-effort, fire-and-forget).
///
/// No-op if the publisher has not been initialized.
pub fn publish(event: OmniEvent) {
    let Some(config) = CONFIG.get() else {
        return;
    };
    let config = config.clone();
    drop(tokio::spawn(async move {
        if let Err(e) = send_event(&config, &event).await {
            tracing::warn!(
                event_type = %event.event_type,
                error = %e,
                "failed to publish OmniEvent"
            );
        } else {
            tracing::debug!(
                event_type = %event.event_type,
                topic = %event.organization_id,
                "published OmniEvent"
            );
        }
    }));
}

// -- Private HTTP helpers --

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    tokens: LoginTokens,
}

#[derive(Debug, Deserialize)]
struct LoginTokens {
    access: AccessToken,
}

#[derive(Debug, Deserialize)]
struct AccessToken {
    token: String,
}

#[derive(Debug, Serialize)]
struct CreateStreamRequest<'a> {
    stream_id: u32,
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateTopicRequest<'a> {
    name: &'a str,
    partitions_count: u32,
    message_expiry: u64,
}

#[derive(Debug, Serialize)]
struct SendMessagesRequest {
    partitioning: Partitioning,
    messages: Vec<IggyMessage>,
}

#[derive(Debug, Serialize)]
struct Partitioning {
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct IggyMessage {
    /// Base64-encoded JSON payload
    payload: String,
}

/// Send an event to the Iggy HTTP API.
///
/// Uses a cached bearer token, re-authenticating on token expiry.
///
/// # Errors
///
/// Returns an error if authentication, stream/topic provisioning, or message delivery fails.
async fn send_event(config: &EventsConfig, event: &OmniEvent) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = cached_token(&client, config).await?;

    if let Err(e) = inner_send(&client, config, event, &token).await {
        // Invalidate the cached token so the next publish re-authenticates
        invalidate_token().await;
        return Err(e);
    }

    Ok(())
}

/// Provision stream/topic and publish the encoded message.
///
/// # Errors
///
/// Returns an error if stream/topic creation or message delivery fails.
async fn inner_send(
    client: &reqwest::Client,
    config: &EventsConfig,
    event: &OmniEvent,
    token: &str,
) -> anyhow::Result<()> {
    // Ensure stream exists
    let stream_resp = client
        .get(format!("{}/streams/{STREAM_NAME}", config.base_url))
        .bearer_auth(token)
        .send()
        .await?;
    if !stream_resp.status().is_success() {
        let _ = client
            .post(format!("{}/streams", config.base_url))
            .bearer_auth(token)
            .json(&CreateStreamRequest {
                stream_id: 1,
                name: STREAM_NAME,
            })
            .send()
            .await?;
    }

    // Ensure per-organization topic exists
    let topic_id = &event.organization_id;
    let topic_resp = client
        .get(format!(
            "{}/streams/{STREAM_NAME}/topics/{topic_id}",
            config.base_url
        ))
        .bearer_auth(token)
        .send()
        .await?;
    if !topic_resp.status().is_success() {
        let _ = client
            .post(format!(
                "{}/streams/{STREAM_NAME}/topics",
                config.base_url
            ))
            .bearer_auth(token)
            .json(&CreateTopicRequest {
                name: topic_id,
                partitions_count: TOPIC_PARTITIONS,
                message_expiry: RETENTION_SECS,
            })
            .send()
            .await?;
    }

    // Encode payload as base64
    let payload_bytes = serde_json::to_vec(event)?;
    let payload_b64 = base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    // Publish message
    let send_resp = client
        .post(format!(
            "{}/streams/{STREAM_NAME}/topics/{topic_id}/messages",
            config.base_url
        ))
        .bearer_auth(token)
        .json(&SendMessagesRequest {
            partitioning: Partitioning { kind: "balanced" },
            messages: vec![IggyMessage {
                payload: payload_b64,
            }],
        })
        .send()
        .await?;

    if !send_resp.status().is_success() {
        let status = send_resp.status();
        let body = send_resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Iggy send failed: {status} - {body}"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_started_event_has_correct_type() {
        let event = build_conversation_started_event("sess-1", "discord", "org-1");
        assert_eq!(event.event_type, "beacon.conversation.started");
        assert_eq!(event.source, "beacon-gateway");
        assert_eq!(event.subject, Some("sess-1".to_string()));
        assert_eq!(event.organization_id, "org-1");
        assert_eq!(event.data["channel"], "discord");
        assert_eq!(event.data["conversationId"], "sess-1");
    }

    #[test]
    fn conversation_ended_event_has_correct_type() {
        let event = build_conversation_ended_event("sess-2", "slack", "org-2");
        assert_eq!(event.event_type, "beacon.conversation.ended");
        assert_eq!(event.source, "beacon-gateway");
        assert_eq!(event.subject, Some("sess-2".to_string()));
        assert_eq!(event.organization_id, "org-2");
        assert_eq!(event.data["channel"], "slack");
        assert_eq!(event.data["conversationId"], "sess-2");
    }

    #[test]
    fn tool_executed_event_has_correct_type() {
        let event = build_tool_executed_event("sess-3", "web_search", true, "org-3");
        assert_eq!(event.event_type, "beacon.tool.executed");
        assert_eq!(event.source, "beacon-gateway");
        assert_eq!(event.subject, Some("sess-3".to_string()));
        assert_eq!(event.organization_id, "org-3");
        assert_eq!(event.data["toolName"], "web_search");
        assert_eq!(event.data["success"], true);
        assert_eq!(event.data["conversationId"], "sess-3");
    }

    #[test]
    fn tool_executed_event_captures_failure() {
        let event = build_tool_executed_event("sess-4", "bash", false, "org-4");
        assert_eq!(event.data["success"], false);
    }
}
