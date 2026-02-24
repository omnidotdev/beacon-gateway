//! Matrix channel adapter using Client-Server API
//!
//! Uses the Matrix Client-Server API with long-polling /sync for receiving messages

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Attachment, AttachmentKind, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Matrix channel adapter
pub struct MatrixChannel {
    homeserver_url: String,
    access_token: String,
    user_id: String,
    client: reqwest::Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
    sync_token: Option<String>,
}

/// Matrix sync response
#[derive(Debug, Deserialize)]
struct SyncResponse {
    next_batch: String,
    rooms: Option<RoomsResponse>,
}

/// Rooms in sync response
#[derive(Debug, Deserialize)]
struct RoomsResponse {
    join: Option<HashMap<String, JoinedRoom>>,
    #[allow(dead_code)]
    invite: Option<HashMap<String, serde_json::Value>>,
}

/// A joined room in sync response
#[derive(Debug, Deserialize)]
struct JoinedRoom {
    timeline: Option<Timeline>,
}

/// Timeline events in a room
#[derive(Debug, Deserialize)]
struct Timeline {
    events: Vec<RoomEvent>,
}

/// A room event
#[derive(Debug, Deserialize)]
struct RoomEvent {
    #[serde(rename = "type")]
    event_type: String,
    event_id: Option<String>,
    sender: String,
    content: EventContent,
    #[serde(default)]
    unsigned: UnsignedData,
}

/// Event content
#[derive(Debug, Deserialize)]
struct EventContent {
    body: Option<String>,
    msgtype: Option<String>,
    /// Media URL (mxc://...)
    url: Option<String>,
    /// MIME type for media
    #[serde(rename = "mimetype")]
    mime_type: Option<String>,
    /// Media info (contains mimetype, size, etc.)
    info: Option<MediaInfo>,
    #[serde(rename = "m.relates_to")]
    relates_to: Option<RelatesTo>,
}

/// Media info for Matrix media messages
#[derive(Debug, Deserialize)]
struct MediaInfo {
    #[serde(rename = "mimetype")]
    mime_type: Option<String>,
    size: Option<u64>,
}

/// Unsigned data (age, `transaction_id`, etc.)
#[derive(Debug, Default, Deserialize)]
struct UnsignedData {
    #[serde(default)]
    transaction_id: Option<String>,
}

/// Relation metadata (for replies, reactions, etc.)
#[derive(Debug, Deserialize)]
struct RelatesTo {
    #[serde(rename = "m.in_reply_to")]
    in_reply_to: Option<InReplyTo>,
    #[allow(dead_code)]
    rel_type: Option<String>,
    #[allow(dead_code)]
    event_id: Option<String>,
    #[allow(dead_code)]
    key: Option<String>,
}

/// Reply reference
#[derive(Debug, Deserialize)]
struct InReplyTo {
    event_id: String,
}

/// Message send request
#[derive(Debug, Serialize)]
struct MessageRequest<'a> {
    msgtype: &'a str,
    body: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    formatted_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "m.relates_to")]
    relates_to: Option<MessageRelatesTo<'a>>,
}

/// Relates-to for outgoing messages
#[derive(Debug, Serialize)]
struct MessageRelatesTo<'a> {
    #[serde(rename = "m.in_reply_to")]
    in_reply_to: InReplyToRef<'a>,
}

/// In-reply-to reference
#[derive(Debug, Serialize)]
struct InReplyToRef<'a> {
    event_id: &'a str,
}

/// Typing notification request
#[derive(Debug, Serialize)]
struct TypingRequest {
    typing: bool,
    timeout: Option<u64>,
}

/// Reaction request
#[derive(Debug, Serialize)]
struct ReactionRequest<'a> {
    #[serde(rename = "m.relates_to")]
    relates_to: ReactionRelatesTo<'a>,
}

/// Reaction relates-to
#[derive(Debug, Serialize)]
struct ReactionRelatesTo<'a> {
    rel_type: &'a str,
    event_id: &'a str,
    key: &'a str,
}

/// Login response (for potential future use)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct LoginResponse {
    user_id: String,
    access_token: String,
    device_id: String,
}

/// Whoami response
#[derive(Debug, Deserialize)]
struct WhoamiResponse {
    user_id: String,
}

impl MatrixChannel {
    /// Create a new Matrix channel adapter
    ///
    /// # Arguments
    ///
    /// * `homeserver_url` - Matrix homeserver URL (e.g., "<https://matrix.org>")
    /// * `access_token` - Access token for authentication
    /// * `user_id` - Full Matrix user ID (e.g., "@bot:matrix.org")
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(homeserver_url: String, access_token: String, user_id: String) -> Self {
        // Normalize homeserver URL (remove trailing slash)
        let homeserver_url = homeserver_url.trim_end_matches('/').to_string();

        Self {
            homeserver_url,
            access_token,
            user_id,
            client: reqwest::Client::new(),
            message_tx: None,
            connected: false,
            sync_token: None,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn with_receiver(
        homeserver_url: String,
        access_token: String,
        user_id: String,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let homeserver_url = homeserver_url.trim_end_matches('/').to_string();

        let channel = Self {
            homeserver_url,
            access_token,
            user_id,
            client: reqwest::Client::new(),
            message_tx: Some(tx),
            connected: false,
            sync_token: None,
        };
        (channel, rx)
    }

    /// Build API endpoint URL
    fn api_url(&self, path: &str) -> String {
        format!("{}/_matrix/client/v3{}", self.homeserver_url, path)
    }

    /// Perform initial sync to get current state
    async fn initial_sync(&mut self) -> Result<()> {
        // Do an initial sync with a filter to skip history
        let url = format!(
            "{}?filter={{\"room\":{{\"timeline\":{{\"limit\":0}}}}}}&timeout=0",
            self.api_url("/sync")
        );

        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Matrix sync request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Matrix initial sync failed: {status} - {body}"
            )));
        }

        let sync: SyncResponse = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Matrix sync parse error: {e}")))?;

        self.sync_token = Some(sync.next_batch);
        tracing::debug!("Matrix initial sync complete");

        Ok(())
    }

    /// Run the sync loop in background
    #[allow(clippy::too_many_lines)]
    fn spawn_sync_loop(&self) {
        let homeserver_url = self.homeserver_url.clone();
        let access_token = self.access_token.clone();
        let user_id = self.user_id.clone();
        let message_tx = self.message_tx.clone();
        let sync_token = self.sync_token.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            let mut current_token = sync_token;

            loop {
                // Build sync URL with long-polling timeout
                let mut url = format!(
                    "{homeserver_url}/_matrix/client/v3/sync?timeout=30000"
                );
                if let Some(token) = &current_token {
                    use std::fmt::Write;
                    let _ = write!(url, "&since={token}");
                }

                let response = match client
                    .get(&url)
                    .bearer_auth(&access_token)
                    .timeout(Duration::from_secs(60))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(error = %e, "Matrix sync request failed, retrying");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                if !response.status().is_success() {
                    tracing::warn!(status = %response.status(), "Matrix sync error, retrying");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }

                let sync: SyncResponse = match response.json().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "Matrix sync parse error, retrying");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };

                current_token = Some(sync.next_batch);

                // Process room events
                if let Some(rooms) = sync.rooms {
                    if let Some(joined) = rooms.join {
                        for (room_id, room) in joined {
                            if let Some(timeline) = room.timeline {
                                for event in timeline.events {
                                    // Skip non-message events
                                    if event.event_type != "m.room.message" {
                                        continue;
                                    }

                                    // Skip our own messages
                                    if event.sender == user_id {
                                        continue;
                                    }

                                    // Skip messages we sent (transaction_id present)
                                    if event.unsigned.transaction_id.is_some() {
                                        continue;
                                    }

                                    let msgtype = event.content.msgtype.as_deref();

                                    // Handle text and media messages
                                    let (content, attachments) = match msgtype {
                                        Some("m.text") => {
                                            (event.content.body.unwrap_or_default(), Vec::new())
                                        }
                                        Some("m.image" | "m.audio" | "m.video" | "m.file") => {
                                            let body = event.content.body.clone().unwrap_or_default();
                                            let mut atts = Vec::new();

                                            if let Some(mxc_url) = &event.content.url {
                                                // Get MIME type from info or content
                                                let mime = event.content.info
                                                    .as_ref()
                                                    .and_then(|i| i.mime_type.clone())
                                                    .or_else(|| event.content.mime_type.clone())
                                                    .unwrap_or_else(|| "application/octet-stream".to_string());

                                                // Convert mxc:// to https:// download URL
                                                let download_url = convert_mxc_to_https(mxc_url, &homeserver_url);

                                                let kind = match msgtype {
                                                    Some("m.image") => AttachmentKind::Image,
                                                    Some("m.audio") => AttachmentKind::Audio,
                                                    Some("m.video") => AttachmentKind::Video,
                                                    _ => AttachmentKind::File,
                                                };

                                                atts.push(Attachment {
                                                    kind,
                                                    url: download_url,
                                                    data: None,
                                                    mime_type: mime,
                                                    filename: Some(body.clone()),
                                                });
                                            }

                                            (body, atts)
                                        }
                                        _ => continue, // Skip other event types
                                    };

                                    let reply_to = event
                                        .content
                                        .relates_to
                                        .and_then(|r| r.in_reply_to)
                                        .map(|r| r.event_id);

                                    let incoming = IncomingMessage {
                                        id: event.event_id.unwrap_or_default(),
                                        channel_id: room_id.clone(),
                                        sender_id: event.sender.clone(),
                                        sender_name: event.sender.clone(),
                                        content,
                                        is_dm: false,
                                        reply_to,
                                        attachments,
                                        thread_id: None,
                                        callback_data: None,
                                    };

                                    if let Some(tx) = &message_tx {
                                        if let Err(e) = tx.send(incoming).await {
                                            tracing::warn!(error = %e, "Failed to forward Matrix message");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    /// Generate a transaction ID for message sending
    fn txn_id() -> String {
        format!("beacon_{}", uuid::Uuid::new_v4())
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &'static str {
        "matrix"
    }

    async fn connect(&mut self) -> Result<()> {
        // Verify credentials with whoami
        let response = self
            .client
            .get(self.api_url("/account/whoami"))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Matrix request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Matrix auth failed: {status} - {body}"
            )));
        }

        let whoami: WhoamiResponse = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Matrix parse error: {e}")))?;

        tracing::info!(
            user_id = %whoami.user_id,
            homeserver = %self.homeserver_url,
            "Matrix authenticated"
        );

        // Do initial sync to get current position
        self.initial_sync().await?;

        // Start sync loop
        self.spawn_sync_loop();

        self.connected = true;
        tracing::info!("Matrix channel connected");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Matrix channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let txn_id = Self::txn_id();
        let url = format!(
            "{}/rooms/{}/send/m.room.message/{}",
            self.api_url(""),
            urlencoding::encode(&message.channel_id),
            txn_id
        );

        let relates_to = message.reply_to.as_ref().map(|event_id| MessageRelatesTo {
            in_reply_to: InReplyToRef { event_id },
        });

        // Build request with optional HTML formatting for code blocks
        let (format, formatted_body) = if message.has_code_blocks() {
            (Some("org.matrix.custom.html"), Some(convert_to_html(&message.content)))
        } else {
            (None, None)
        };

        let request = MessageRequest {
            msgtype: "m.text",
            body: &message.content,
            format,
            formatted_body,
            relates_to,
        };

        let response = self
            .client
            .put(&url)
            .bearer_auth(&self.access_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Matrix send failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Matrix send failed: {status} - {body}"
            )));
        }

        tracing::debug!(room = %message.channel_id, "Matrix message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, channel_id: &str) -> Result<()> {
        let url = format!(
            "{}/rooms/{}/typing/{}",
            self.api_url(""),
            urlencoding::encode(channel_id),
            urlencoding::encode(&self.user_id)
        );

        let request = TypingRequest {
            typing: true,
            timeout: Some(30000),
        };

        let response = self
            .client
            .put(&url)
            .bearer_auth(&self.access_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Matrix typing failed: {e}")))?;

        if !response.status().is_success() {
            tracing::debug!(
                status = %response.status(),
                "Matrix typing indicator failed"
            );
        }

        Ok(())
    }

    async fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<()> {
        let txn_id = Self::txn_id();
        let url = format!(
            "{}/rooms/{}/send/m.reaction/{}",
            self.api_url(""),
            urlencoding::encode(channel_id),
            txn_id
        );

        let request = ReactionRequest {
            relates_to: ReactionRelatesTo {
                rel_type: "m.annotation",
                event_id: message_id,
                key: emoji,
            },
        };

        let response = self
            .client
            .put(&url)
            .bearer_auth(&self.access_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Matrix reaction failed: {e}")))?;

        if !response.status().is_success() {
            tracing::debug!(
                status = %response.status(),
                "Matrix reaction failed"
            );
        }

        tracing::debug!(room = %channel_id, message_id, emoji, "Matrix reaction added");
        Ok(())
    }

    async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        // Matrix doesn't have a direct "remove reaction" API
        // Reactions are removed by redacting the reaction event
        // This would require tracking reaction event IDs
        tracing::debug!(
            room = %channel_id,
            message_id,
            emoji,
            "Matrix reaction removal not implemented (requires redaction)"
        );
        Ok(())
    }
}

/// Convert Matrix mxc:// URL to HTTPS download URL
///
/// `mxc://server/media_id` -> `https://homeserver/_matrix/media/v3/download/server/media_id`
fn convert_mxc_to_https(mxc_url: &str, homeserver_url: &str) -> Option<String> {
    if !mxc_url.starts_with("mxc://") {
        return None;
    }

    let path = mxc_url.strip_prefix("mxc://")?;
    let homeserver = homeserver_url.trim_end_matches('/');

    Some(format!("{homeserver}/_matrix/media/v3/download/{path}"))
}

/// Convert markdown content to HTML for Matrix `formatted_body`
fn convert_to_html(content: &str) -> String {
    use std::fmt::Write;

    let mut html = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();

    for line in content.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block
                let escaped_code = html_escape(&code_content);
                if code_lang.is_empty() {
                    let _ = write!(html, "<pre><code>{}</code></pre>", escaped_code.trim());
                } else {
                    let _ = write!(
                        html,
                        "<pre><code class=\"language-{}\">{}</code></pre>",
                        html_escape(&code_lang),
                        escaped_code.trim()
                    );
                }
                code_content.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start code block
                code_lang = line.trim_start_matches('`').to_string();
                in_code_block = true;
            }
        } else if in_code_block {
            if !code_content.is_empty() {
                code_content.push('\n');
            }
            code_content.push_str(line);
        } else {
            // Regular text
            if !html.is_empty() && !html.ends_with("</pre>") {
                html.push_str("<br>");
            }
            html.push_str(&html_escape(line));
        }
    }

    html
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
