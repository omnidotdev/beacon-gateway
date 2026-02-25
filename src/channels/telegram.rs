//! Telegram channel adapter
//!
//! Uses webhooks for receiving messages and Bot API for sending

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Telegram Bot API base URL
const API_BASE: &str = "https://api.telegram.org/bot";

/// Per-chat rate limiter for Telegram API edit operations
#[derive(Debug, Clone)]
pub struct TelegramRateLimiter {
    /// Minimum interval between edits per chat
    interval: Duration,
    /// Last edit timestamp per chat
    last_edit: Arc<Mutex<HashMap<String, Instant>>>,
}

impl TelegramRateLimiter {
    /// Create a rate limiter with the given minimum interval between edits per chat
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_edit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if an edit is allowed for the given chat. Returns true if allowed.
    pub fn check(&self, chat_id: &str) -> bool {
        let mut map = self.last_edit.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        if let Some(last) = map.get(chat_id) {
            if now.duration_since(*last) < self.interval {
                return false;
            }
        }

        map.insert(chat_id.to_string(), now);
        true
    }

    /// Record a 429 response — push the effective interval forward for this chat
    pub fn backoff(&self, chat_id: &str) {
        let mut map = self.last_edit.lock().unwrap_or_else(|e| e.into_inner());
        let future = Instant::now() + self.interval;
        map.insert(chat_id.to_string(), future);
    }
}

/// Telegram channel adapter
#[derive(Clone)]
pub struct TelegramChannel {
    token: String,
    client: Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
}

impl TelegramChannel {
    /// Create a new Telegram channel adapter
    #[must_use]
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: Client::new(),
            message_tx: None,
            connected: false,
        }
    }

    /// Create with a message receiver for polling mode
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(token: String) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            token,
            client: Client::new(),
            message_tx: Some(tx),
            connected: false,
        };
        (channel, rx)
    }

    /// Spawn a background task that polls Telegram's getUpdates API
    ///
    /// Polls every `interval` and forwards received messages into the mpsc channel.
    /// Deletes any existing webhook before starting to avoid conflicts.
    pub fn start_polling(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let token = self.token.clone();
        let client = self.client.clone();
        let tx = self
            .message_tx
            .clone()
            .expect("start_polling requires a message_tx (use with_receiver)");

        tokio::spawn(async move {
            // Delete any existing webhook so getUpdates works
            let delete_url = format!("{API_BASE}{token}/deleteWebhook");
            if let Err(e) = client.post(&delete_url).send().await {
                tracing::warn!(error = %e, "failed to delete Telegram webhook before polling");
            }

            let mut offset: Option<i64> = None;

            loop {
                let url = format!("{API_BASE}{token}/getUpdates");
                let mut params = serde_json::json!({
                    "timeout": 30,
                    "allowed_updates": ["message"],
                });
                if let Some(off) = offset {
                    params["offset"] = serde_json::json!(off);
                }

                match client.post(&url).json(&params).send().await {
                    Ok(resp) => {
                        if let Ok(body) = resp.text().await {
                            if let Ok(updates) = serde_json::from_str::<GetUpdatesResponse>(&body) {
                                for update in &updates.result {
                                    // Advance offset past this update
                                    offset = Some(update.update_id + 1);

                                    if let Some(msg) = update_to_incoming(update) {
                                        if let Err(e) = tx.send(msg).await {
                                            tracing::warn!(error = %e, "failed to forward Telegram message");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Telegram getUpdates error");
                    }
                }

                tokio::time::sleep(interval).await;
            }
        })
    }

    /// Send a message to a chat
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_message(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<()> {
        let url = format!("{API_BASE}{}/sendMessage", self.token);

        // Use MarkdownV2 which has better support for code blocks
        let request = SendMessageRequest {
            chat_id,
            text: text.to_string(),
            parse_mode: Some("MarkdownV2".to_string()),
            reply_to_message_id: reply_to,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

        if !response.status().is_success() {
            // If MarkdownV2 fails (due to escaping issues), retry with plain text
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            // Try again without parse_mode
            let fallback_request = SendMessageRequest {
                chat_id,
                text: text.to_string(),
                parse_mode: None,
                reply_to_message_id: reply_to,
            };

            let fallback_response = self
                .client
                .post(&url)
                .json(&fallback_request)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

            if !fallback_response.status().is_success() {
                return Err(Error::Channel(format!(
                    "Telegram API error: {status} - {body}"
                )));
            }
        }

        tracing::debug!(chat_id, "Telegram message sent");
        Ok(())
    }

    /// Set webhook URL for receiving updates
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn set_webhook(&self, url: &str) -> Result<()> {
        let api_url = format!("{API_BASE}{}/setWebhook", self.token);

        let request = SetWebhookRequest {
            url: url.to_string(),
            allowed_updates: Some(vec!["message".to_string()]),
        };

        let response = self
            .client
            .post(&api_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram setWebhook error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram setWebhook error: {status} - {body}"
            )));
        }

        tracing::info!(url, "Telegram webhook set");
        Ok(())
    }

    /// Delete webhook (switch to polling mode)
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn delete_webhook(&self) -> Result<()> {
        let url = format!("{API_BASE}{}/deleteWebhook", self.token);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram deleteWebhook error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram deleteWebhook error: {status} - {body}"
            )));
        }

        tracing::info!("Telegram webhook deleted");
        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn connect(&mut self) -> Result<()> {
        // Telegram uses webhooks, so "connect" just validates the token
        let url = format!("{API_BASE}{}/getMe", self.token);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram getMe error: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Channel("Invalid Telegram bot token".to_string()));
        }

        self.connected = true;
        tracing::info!("Telegram channel connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Telegram channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let chat_id: i64 = message
            .channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;

        let reply_to = message.reply_to.as_ref().and_then(|id| id.parse().ok());

        self.send_message(chat_id, &message.content, reply_to).await
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, channel_id: &str) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;

        let url = format!("{API_BASE}{}/sendChatAction", self.token);

        let request = SendChatActionRequest {
            chat_id,
            action: "typing".to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram sendChatAction error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram sendChatAction error: {status} - {body}"
            )));
        }

        tracing::debug!(chat_id, "Telegram typing indicator sent");
        Ok(())
    }

    // TODO: Telegram Bot API has setMessageReaction (added in Bot API 7.2) but it requires
    // the bot to be an admin in the chat and only works with specific emoji. For now, we
    // use the default no-op implementation. To implement:
    // POST /setMessageReaction { chat_id, message_id, reaction: [{ type: "emoji", emoji: "👀" }] }
}

/// Telegram sendMessage request
#[derive(Serialize)]
struct SendMessageRequest {
    chat_id: i64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
}

/// Telegram setWebhook request
#[derive(Serialize)]
struct SetWebhookRequest {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_updates: Option<Vec<String>>,
}

/// Telegram sendChatAction request
#[derive(Serialize)]
struct SendChatActionRequest {
    chat_id: i64,
    action: String,
}

/// Telegram API response wrapper
#[derive(Deserialize)]
#[allow(dead_code)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

/// Response from Telegram getUpdates API
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    #[allow(dead_code)]
    ok: bool,
    result: Vec<PollingUpdate>,
}

/// A single update from getUpdates
#[derive(Debug, Deserialize)]
struct PollingUpdate {
    update_id: i64,
    message: Option<PollingMessage>,
}

/// Message from a polling update
#[derive(Debug, Deserialize)]
struct PollingMessage {
    message_id: i64,
    chat: PollingChat,
    from: Option<PollingUser>,
    text: Option<String>,
    caption: Option<String>,
}

/// Chat info from polling
#[derive(Debug, Deserialize)]
struct PollingChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

/// User info from polling
#[derive(Debug, Deserialize)]
struct PollingUser {
    id: i64,
    is_bot: bool,
    first_name: String,
}

/// Convert a polling update into an `IncomingMessage`
fn update_to_incoming(update: &PollingUpdate) -> Option<IncomingMessage> {
    let msg = update.message.as_ref()?;
    let text = msg.text.clone().or_else(|| msg.caption.clone())?;

    // Skip bot messages
    if msg.from.as_ref().is_some_and(|u| u.is_bot) {
        return None;
    }

    let sender_id = msg
        .from
        .as_ref()
        .map_or_else(|| msg.chat.id.to_string(), |u| u.id.to_string());

    let sender_name = msg
        .from
        .as_ref()
        .map_or_else(|| "Unknown".to_string(), |u| u.first_name.clone());

    let is_dm = msg.chat.chat_type == "private";

    Some(IncomingMessage {
        id: msg.message_id.to_string(),
        channel_id: msg.chat.id.to_string(),
        sender_id,
        sender_name,
        content: text,
        is_dm,
        reply_to: None,
        attachments: vec![],
        thread_id: None,
        callback_data: None,
    })
}
