//! `OmniEvent` publishing via Iggy HTTP REST API
//!
//! Publishes lifecycle events to the `omni-events` Iggy stream.
//! Publishing is best-effort — errors are logged and never propagate to callers.
//!
//! Initialize once at startup with [`init_publisher`], then call [`publish`] anywhere.

use std::sync::OnceLock;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
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
/// # Errors
///
/// Returns an error if login, stream/topic provisioning, or message delivery fails.
async fn send_event(config: &EventsConfig, event: &OmniEvent) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Authenticate and obtain bearer token
    let login_resp: LoginResponse = client
        .post(format!("{}/users/login", config.base_url))
        .json(&LoginRequest {
            username: &config.username,
            password: &config.password,
        })
        .send()
        .await?
        .json()
        .await?;
    let token = login_resp.tokens.access.token;

    // Ensure stream exists
    let stream_resp = client
        .get(format!("{}/streams/{STREAM_NAME}", config.base_url))
        .bearer_auth(&token)
        .send()
        .await?;
    if !stream_resp.status().is_success() {
        let _ = client
            .post(format!("{}/streams", config.base_url))
            .bearer_auth(&token)
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
        .bearer_auth(&token)
        .send()
        .await?;
    if !topic_resp.status().is_success() {
        let _ = client
            .post(format!(
                "{}/streams/{STREAM_NAME}/topics",
                config.base_url
            ))
            .bearer_auth(&token)
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
    let payload_b64 =
        base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    // Publish message
    let send_resp = client
        .post(format!(
            "{}/streams/{STREAM_NAME}/topics/{topic_id}/messages",
            config.base_url
        ))
        .bearer_auth(&token)
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
