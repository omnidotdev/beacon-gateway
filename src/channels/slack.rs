//! Slack channel adapter using Web API
//!
//! Uses Slack's Web API for sending messages.
//! For receiving messages, use Slack's Events API with a webhook endpoint.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Attachment, Channel, ChannelCapability, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

const SLACK_API_URL: &str = "https://slack.com/api";

/// Slack channel adapter
pub struct SlackChannel {
    bot_token: String,
    client: reqwest::Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    bot_user_id: Option<String>,
    connected: bool,
}

/// Slack API response wrapper
#[derive(Debug, Deserialize)]
struct SlackResponse<T> {
    ok: bool,
    error: Option<String>,
    #[serde(flatten)]
    data: Option<T>,
}

/// Auth test response
#[derive(Debug, Deserialize)]
struct AuthTestResponse {
    user_id: String,
    team: String,
    bot_id: Option<String>,
}

/// Chat post message request (simple text)
#[derive(Debug, Serialize)]
struct PostMessageRequest<'a> {
    channel: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<&'a str>,
}

/// Chat post message request with blocks
#[derive(Debug, Serialize)]
struct PostMessageWithBlocksRequest<'a> {
    channel: &'a str,
    text: &'a str,
    blocks: Vec<SlackBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<&'a str>,
}

/// Slack Block Kit block
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum SlackBlock {
    #[serde(rename = "section")]
    Section { text: SlackText },
    #[serde(rename = "divider")]
    Divider {},
}

/// Slack Block Kit text object
#[derive(Debug, Serialize)]
struct SlackText {
    #[serde(rename = "type")]
    text_type: &'static str,
    text: String,
}

/// Reaction add/remove request
#[derive(Debug, Serialize)]
struct ReactionRequest<'a> {
    channel: &'a str,
    timestamp: &'a str,
    name: &'a str,
}

impl SlackChannel {
    /// Create a new Slack channel adapter
    ///
    /// # Arguments
    ///
    /// * `bot_token` - Slack bot OAuth token (xoxb-...)
    #[must_use]
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            client: reqwest::Client::new(),
            message_tx: None,
            bot_user_id: None,
            connected: false,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(bot_token: String) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            bot_token,
            client: reqwest::Client::new(),
            message_tx: Some(tx),
            bot_user_id: None,
            connected: false,
        };
        (channel, rx)
    }

    /// Process an incoming Slack event (from Events API webhook)
    ///
    /// Call this from your webhook handler when receiving events
    ///
    /// # Errors
    ///
    /// Returns error if message forwarding fails
    pub async fn handle_event(&self, event: &SlackEvent) -> Result<()> {
        if let SlackEventType::Message(msg) = &event.event {
            // Skip bot messages
            if msg.bot_id.is_some() {
                return Ok(());
            }

            // Skip if it's our own message
            if let Some(ref bot_id) = self.bot_user_id {
                if msg.user.as_deref() == Some(bot_id) {
                    return Ok(());
                }
            }

            // Parse file attachments
            let attachments = msg
                .files
                .as_ref()
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|f| {
                            let mime_type = f
                                .mimetype
                                .clone()
                                .unwrap_or_else(|| "application/octet-stream".to_string());
                            let url = f
                                .url_private_download
                                .clone()
                                .or_else(|| f.url_private.clone())?;
                            Some(Attachment::from_url(url, mime_type, f.name.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let incoming = IncomingMessage {
                id: msg.ts.clone().unwrap_or_default(),
                channel_id: msg.channel.clone(),
                sender_id: msg.user.clone().unwrap_or_default(),
                sender_name: msg.user.clone().unwrap_or_default(),
                content: msg.text.clone().unwrap_or_default(),
                is_dm: msg.channel_type.as_deref() == Some("im"),
                reply_to: msg.thread_ts.clone(),
                attachments,
                thread_id: None,
                callback_data: None,
            };

            if let Some(tx) = &self.message_tx {
                tx.send(incoming)
                    .await
                    .map_err(|e| Error::Channel(format!("Failed to forward message: {e}")))?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn capabilities(&self) -> &'static [ChannelCapability] {
        &[ChannelCapability::Reactions]
    }

    async fn connect(&mut self) -> Result<()> {
        // Test authentication
        let response = self
            .client
            .post(format!("{SLACK_API_URL}/auth.test"))
            .bearer_auth(&self.bot_token)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Slack request failed: {e}")))?;

        let auth: SlackResponse<AuthTestResponse> = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Slack parse error: {e}")))?;

        if !auth.ok {
            return Err(Error::Channel(format!(
                "Slack auth failed: {}",
                auth.error.unwrap_or_default()
            )));
        }

        if let Some(data) = auth.data {
            self.bot_user_id = Some(data.user_id.clone());
            tracing::info!(
                user_id = %data.user_id,
                team = %data.team,
                bot_id = ?data.bot_id,
                "Slack authenticated"
            );
        }

        self.connected = true;
        tracing::info!("Slack channel connected");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Slack channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let response = if message.has_code_blocks() {
            // Build blocks for rich content
            let mut blocks = Vec::new();
            let code_blocks = message.extract_code_blocks();
            let plain_content = remove_code_blocks(&message.content);

            // Add plain text section if present
            if !plain_content.trim().is_empty() {
                blocks.push(SlackBlock::Section {
                    text: SlackText {
                        text_type: "mrkdwn",
                        text: plain_content.trim().to_string(),
                    },
                });
            }

            // Add code blocks as sections with markdown formatting
            for (lang, code) in code_blocks {
                // Slack has a 3000 char limit per text block
                let truncated = if code.len() > 2900 {
                    format!("{}...\n(truncated)", &code[..2900])
                } else {
                    code
                };

                // Add divider before code blocks if there was preceding content
                if !blocks.is_empty() {
                    blocks.push(SlackBlock::Divider {});
                }

                blocks.push(SlackBlock::Section {
                    text: SlackText {
                        text_type: "mrkdwn",
                        text: format!("```{truncated}```"),
                    },
                });

                // Add language hint after code block
                if !lang.is_empty() {
                    blocks.push(SlackBlock::Section {
                        text: SlackText {
                            text_type: "mrkdwn",
                            text: format!("_({lang})_"),
                        },
                    });
                }
            }

            let request = PostMessageWithBlocksRequest {
                channel: &message.channel_id,
                text: &message.content, // Fallback for notifications
                blocks,
                thread_ts: message.reply_to.as_deref(),
            };

            self.client
                .post(format!("{SLACK_API_URL}/chat.postMessage"))
                .bearer_auth(&self.bot_token)
                .json(&request)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Slack request failed: {e}")))?
        } else {
            let request = PostMessageRequest {
                channel: &message.channel_id,
                text: &message.content,
                thread_ts: message.reply_to.as_deref(),
            };

            self.client
                .post(format!("{SLACK_API_URL}/chat.postMessage"))
                .bearer_auth(&self.bot_token)
                .json(&request)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Slack request failed: {e}")))?
        };

        let result: SlackResponse<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Slack parse error: {e}")))?;

        if !result.ok {
            return Err(Error::Channel(format!(
                "Slack send failed: {}",
                result.error.unwrap_or_default()
            )));
        }

        tracing::debug!(channel = %message.channel_id, "Slack message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, channel_id: &str) -> Result<()> {
        // Slack's typing indicator requires Socket Mode or RTM API
        // The Web API doesn't have a direct typing endpoint that works without Socket Mode
        // We log at debug level and return Ok since this is a best-effort feature
        tracing::debug!(channel = %channel_id, "Slack typing indicator (not supported via Web API)");
        Ok(())
    }

    async fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<()> {
        // Slack expects emoji name without colons (e.g., "eyes" not ":eyes:")
        let name = emoji.trim_matches(':');

        let request = ReactionRequest {
            channel: channel_id,
            timestamp: message_id,
            name,
        };

        let response = self
            .client
            .post(format!("{SLACK_API_URL}/reactions.add"))
            .bearer_auth(&self.bot_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Slack reaction request failed: {e}")))?;

        let result: SlackResponse<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Slack reaction parse error: {e}")))?;

        if !result.ok {
            // Don't fail if already reacted (common case)
            if result.error.as_deref() != Some("already_reacted") {
                return Err(Error::Channel(format!(
                    "Slack add reaction failed: {}",
                    result.error.unwrap_or_default()
                )));
            }
        }

        tracing::debug!(channel = %channel_id, message_id, emoji, "Slack reaction added");
        Ok(())
    }

    async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        // Slack expects emoji name without colons
        let name = emoji.trim_matches(':');

        let request = ReactionRequest {
            channel: channel_id,
            timestamp: message_id,
            name,
        };

        let response = self
            .client
            .post(format!("{SLACK_API_URL}/reactions.remove"))
            .bearer_auth(&self.bot_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Slack reaction request failed: {e}")))?;

        let result: SlackResponse<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Slack reaction parse error: {e}")))?;

        if !result.ok {
            // Don't fail if reaction doesn't exist
            if result.error.as_deref() != Some("no_reaction") {
                return Err(Error::Channel(format!(
                    "Slack remove reaction failed: {}",
                    result.error.unwrap_or_default()
                )));
            }
        }

        tracing::debug!(channel = %channel_id, message_id, emoji, "Slack reaction removed");
        Ok(())
    }
}

/// Slack event from Events API
#[derive(Debug, Deserialize)]
pub struct SlackEvent {
    /// Event type
    #[serde(rename = "type")]
    pub event_type: String,
    /// The actual event
    pub event: SlackEventType,
}

/// Slack event types
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum SlackEventType {
    /// Message event
    #[serde(rename = "message")]
    Message(SlackMessageEvent),
    /// App mention
    #[serde(rename = "app_mention")]
    AppMention(SlackMessageEvent),
    /// Other events (ignored)
    #[serde(other)]
    Other,
}

/// Slack message event
#[derive(Debug, Deserialize)]
pub struct SlackMessageEvent {
    /// Channel ID
    pub channel: String,
    /// Message timestamp (unique ID)
    pub ts: Option<String>,
    /// User ID
    pub user: Option<String>,
    /// Message text
    pub text: Option<String>,
    /// Thread timestamp
    pub thread_ts: Option<String>,
    /// Bot ID (if from a bot)
    pub bot_id: Option<String>,
    /// Channel type
    pub channel_type: Option<String>,
    /// File attachments
    pub files: Option<Vec<SlackFile>>,
}

/// Slack file attachment
#[derive(Debug, Clone, Deserialize)]
pub struct SlackFile {
    /// File ID
    pub id: String,
    /// Filename
    pub name: Option<String>,
    /// MIME type
    pub mimetype: Option<String>,
    /// Private download URL (requires auth)
    pub url_private: Option<String>,
    /// Direct download URL
    pub url_private_download: Option<String>,
    /// File size in bytes
    pub size: Option<u64>,
}

/// Remove code blocks from content, leaving other text
fn remove_code_blocks(content: &str) -> String {
    let mut result = String::new();
    let mut in_block = false;

    for line in content.lines() {
        if line.starts_with("```") {
            in_block = !in_block;
        } else if !in_block {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
    }

    result
}
