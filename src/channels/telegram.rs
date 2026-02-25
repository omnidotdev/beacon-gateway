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

use super::{Channel, ChannelCapability, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Telegram Bot API base URL
const API_BASE: &str = "https://api.telegram.org/bot";

/// Telegram file download base URL
const FILE_BASE: &str = "https://api.telegram.org/file/bot";

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

/// Default dedup TTL (5 minutes)
const DEDUP_TTL_SECS: u64 = 300;

/// Maximum dedup cache entries
const DEDUP_MAX_ENTRIES: usize = 2000;

/// Telegram update deduplication cache
///
/// Prevents processing the same webhook update or polling result twice.
/// Uses a TTL-based eviction strategy with a hard cap on entries.
#[derive(Debug)]
pub struct UpdateDedup {
    cache: HashMap<String, Instant>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for UpdateDedup {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(DEDUP_TTL_SECS),
            max_entries: DEDUP_MAX_ENTRIES,
        }
    }
}

impl UpdateDedup {
    /// Check if the given key has been seen recently.
    ///
    /// Returns `true` if this is a duplicate (already seen within TTL).
    /// Returns `false` on first sight and records the key.
    pub fn is_duplicate(&mut self, key: &str) -> bool {
        let now = Instant::now();

        // Evict expired entries periodically (when at capacity)
        if self.cache.len() >= self.max_entries {
            self.cache.retain(|_, ts| now.duration_since(*ts) < self.ttl);
        }

        // If still at capacity after eviction, remove oldest entry
        if self.cache.len() >= self.max_entries {
            if let Some(oldest_key) = self
                .cache
                .iter()
                .min_by_key(|(_, ts)| *ts)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&oldest_key);
            }
        }

        if let Some(ts) = self.cache.get(key) {
            if now.duration_since(*ts) < self.ttl {
                return true;
            }
        }

        self.cache.insert(key.to_string(), now);
        false
    }
}

/// Default streaming edit interval (1000ms)
const DEFAULT_STREAM_INTERVAL_MS: u64 = 1000;

/// Telegram channel adapter
#[derive(Clone)]
pub struct TelegramChannel {
    token: String,
    client: Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
    /// Rate limiter for streaming edit operations
    rate_limiter: TelegramRateLimiter,
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
            rate_limiter: TelegramRateLimiter::new(Duration::from_millis(
                DEFAULT_STREAM_INTERVAL_MS,
            )),
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
            rate_limiter: TelegramRateLimiter::new(Duration::from_millis(
                DEFAULT_STREAM_INTERVAL_MS,
            )),
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
            let mut dedup = UpdateDedup::default();

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

                                    // Dedup check
                                    let key = format!("poll:{}", update.update_id);
                                    if dedup.is_duplicate(&key) {
                                        continue;
                                    }

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
    /// Uses HTML parse mode with plain-text fallback.
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_message(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<()> {
        let url = format!("{API_BASE}{}/sendMessage", self.token);

        let html_text = super::telegram_html::markdown_to_telegram_html(text);
        let request = SendMessageRequest {
            chat_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
            reply_to_message_id: reply_to,
            message_thread_id: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

        if !response.status().is_success() {
            // If HTML parse fails, retry with plain text
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            let fallback_request = SendMessageRequest {
                chat_id,
                text: text.to_string(),
                parse_mode: None,
                reply_to_message_id: reply_to,
                message_thread_id: None,
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

    /// Send a message and return the platform message ID
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails or the response lacks a message ID
    pub async fn send_message_returning_id(
        &self,
        chat_id: i64,
        text: &str,
        reply_to: Option<i64>,
        thread_id: Option<i64>,
    ) -> Result<i64> {
        let url = format!("{API_BASE}{}/sendMessage", self.token);

        let html_text = super::telegram_html::markdown_to_telegram_html(text);
        let request = SendMessageRequest {
            chat_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

        let body = response
            .text()
            .await
            .map_err(|e| Error::Channel(format!("Telegram response read error: {e}")))?;

        let parsed: TelegramResponse<SentMessage> = serde_json::from_str(&body)
            .map_err(|e| Error::Channel(format!("Telegram response parse error: {e}")))?;

        parsed
            .result
            .map(|m| m.message_id)
            .ok_or_else(|| Error::Channel(format!("Telegram API error: {}", parsed.description.unwrap_or_default())))
    }

    /// Edit an existing message's text
    ///
    /// Converts markdown to Telegram HTML with plain-text fallback.
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<()> {
        let url = format!("{API_BASE}{}/editMessageText", self.token);

        let html_text = super::telegram_html::markdown_to_telegram_html(text);
        let request = EditMessageTextRequest {
            chat_id,
            message_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram editMessageText error: {e}")))?;

        if !response.status().is_success() {
            // Fallback to plain text on parse error
            let fallback = EditMessageTextRequest {
                chat_id,
                message_id,
                text: text.to_string(),
                parse_mode: None,
            };

            let fallback_resp = self
                .client
                .post(&url)
                .json(&fallback)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Telegram editMessageText error: {e}")))?;

            if fallback_resp.status().as_u16() == 429 {
                self.rate_limiter.backoff(&chat_id.to_string());
            }

            if !fallback_resp.status().is_success() {
                let body = fallback_resp.text().await.unwrap_or_default();
                return Err(Error::Channel(format!(
                    "Telegram editMessageText error: {body}"
                )));
            }
        }

        Ok(())
    }

    /// Delete a message by ID
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn delete_message_by_id(&self, chat_id: i64, message_id: i64) -> Result<()> {
        let url = format!("{API_BASE}{}/deleteMessage", self.token);

        let request = DeleteMessageRequest {
            chat_id,
            message_id,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram deleteMessage error: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram deleteMessage error: {body}"
            )));
        }

        Ok(())
    }

    /// Download a file from Telegram by `file_id`.
    ///
    /// Calls `getFile` to get the file path, then downloads from
    /// `https://api.telegram.org/file/bot{token}/{file_path}`.
    ///
    /// # Errors
    ///
    /// Returns error if the API request or download fails
    pub async fn download_file(&self, file_id: &str) -> Result<(Vec<u8>, String)> {
        let url = format!("{API_BASE}{}/getFile", self.token);

        let request = GetFileRequest {
            file_id: file_id.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram getFile error: {e}")))?;

        let body = response
            .text()
            .await
            .map_err(|e| Error::Channel(format!("Telegram getFile response read error: {e}")))?;

        let parsed: TelegramResponse<TelegramFile> = serde_json::from_str(&body)
            .map_err(|e| Error::Channel(format!("Telegram getFile parse error: {e}")))?;

        let file = parsed
            .result
            .ok_or_else(|| Error::Channel(format!(
                "Telegram getFile error: {}",
                parsed.description.unwrap_or_default()
            )))?;

        let file_path = file.file_path.ok_or_else(|| {
            Error::Channel("Telegram getFile returned no file_path".to_string())
        })?;

        let download_url = format!("{FILE_BASE}{}/{file_path}", self.token);
        let data = self
            .client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram file download error: {e}")))?
            .bytes()
            .await
            .map_err(|e| Error::Channel(format!("Telegram file download read error: {e}")))?;

        Ok((data.to_vec(), file_path))
    }

    /// Sync bot commands with Telegram via `setMyCommands`
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn sync_commands(&self, commands: &[BotCommand]) -> Result<()> {
        let url = format!("{API_BASE}{}/setMyCommands", self.token);

        let request = SetMyCommandsRequest {
            commands: commands.to_vec(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram setMyCommands error: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram setMyCommands error: {body}"
            )));
        }

        tracing::info!(count = commands.len(), "Telegram bot commands synced");
        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn capabilities(&self) -> &'static [ChannelCapability] {
        &[
            ChannelCapability::MessageEdit,
            ChannelCapability::MessageDelete,
            ChannelCapability::Streaming,
            ChannelCapability::Reactions,
            ChannelCapability::ForumTopics,
        ]
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

        // If edit_target is set, edit instead of sending new
        if let Some(ref target) = message.edit_target
            && let Ok(msg_id) = target.parse::<i64>()
        {
            return self.edit_message_text(chat_id, msg_id, &message.content).await;
        }

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

    async fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        let url = format!("{API_BASE}{}/setMessageReaction", self.token);
        let request = SetMessageReactionRequest {
            chat_id,
            message_id: msg_id,
            reaction: vec![ReactionEmoji {
                reaction_type: "emoji".to_string(),
                emoji: emoji.to_string(),
            }],
            is_big: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram setMessageReaction error: {e}")))?;

        if !response.status().is_success() {
            // Graceful degradation — bot may not be admin
            tracing::warn!(
                chat_id,
                msg_id,
                emoji,
                "Telegram reaction failed (bot may not have permission)"
            );
        }

        Ok(())
    }

    async fn remove_reaction(&self, channel_id: &str, message_id: &str, _emoji: &str) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        let url = format!("{API_BASE}{}/setMessageReaction", self.token);
        let request = SetMessageReactionRequest {
            chat_id,
            message_id: msg_id,
            reaction: vec![],
            is_big: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram setMessageReaction error: {e}")))?;

        if !response.status().is_success() {
            tracing::warn!(chat_id, msg_id, "Telegram remove reaction failed");
        }

        Ok(())
    }

    async fn send_streaming_start(
        &self,
        channel_id: &str,
        initial_text: &str,
        reply_to: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<String> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let reply = reply_to.and_then(|id| id.parse().ok());
        let thread = thread_id.and_then(|id| id.parse().ok());

        let msg_id = self
            .send_message_returning_id(chat_id, initial_text, reply, thread)
            .await?;

        Ok(msg_id.to_string())
    }

    async fn send_streaming_update(
        &self,
        channel_id: &str,
        message_id: &str,
        text: &str,
    ) -> Result<()> {
        // Skip if throttled
        if !self.rate_limiter.check(channel_id) {
            return Ok(());
        }

        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        self.edit_message_text(chat_id, msg_id, text).await
    }

    async fn send_streaming_end(
        &self,
        channel_id: &str,
        message_id: &str,
        final_text: &str,
    ) -> Result<()> {
        // Always edit on final (bypass rate limiter) to ensure final text lands
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        self.edit_message_text(chat_id, msg_id, final_text).await
    }

    async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        new_content: &str,
    ) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        self.edit_message_text(chat_id, msg_id, new_content).await
    }

    async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        self.delete_message_by_id(chat_id, msg_id).await
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
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

/// Telegram editMessageText request
#[derive(Serialize)]
struct EditMessageTextRequest {
    chat_id: i64,
    message_id: i64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<String>,
}

/// Telegram deleteMessage request
#[derive(Serialize)]
struct DeleteMessageRequest {
    chat_id: i64,
    message_id: i64,
}

/// Telegram setMessageReaction request
#[derive(Serialize)]
struct SetMessageReactionRequest {
    chat_id: i64,
    message_id: i64,
    reaction: Vec<ReactionEmoji>,
    is_big: bool,
}

/// A single emoji reaction
#[derive(Serialize)]
struct ReactionEmoji {
    #[serde(rename = "type")]
    reaction_type: String,
    emoji: String,
}

/// Telegram getFile request
#[derive(Serialize)]
struct GetFileRequest {
    file_id: String,
}

/// File metadata from Telegram getFile response
#[derive(Debug, Deserialize)]
struct TelegramFile {
    #[allow(dead_code)]
    file_id: String,
    file_path: Option<String>,
}

/// Telegram setMyCommands request
#[derive(Serialize)]
struct SetMyCommandsRequest {
    commands: Vec<BotCommand>,
}

/// A bot command for Telegram's command menu
#[derive(Debug, Clone, Serialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

/// Response from sendMessage containing the sent message
#[derive(Deserialize)]
struct SentMessage {
    message_id: i64,
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
    message_thread_id: Option<i64>,
    photo: Option<Vec<PollingPhotoSize>>,
    document: Option<PollingDocument>,
    voice: Option<PollingVoice>,
    audio: Option<PollingAudio>,
    video: Option<PollingVideo>,
    reply_to_message: Option<Box<serde_json::Value>>,
}

/// Photo size from polling
#[derive(Debug, Deserialize)]
struct PollingPhotoSize {
    file_id: String,
}

/// Document from polling
#[derive(Debug, Deserialize)]
struct PollingDocument {
    file_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
}

/// Voice message from polling
#[derive(Debug, Deserialize)]
struct PollingVoice {
    file_id: String,
    mime_type: Option<String>,
}

/// Audio from polling
#[derive(Debug, Deserialize)]
struct PollingAudio {
    file_id: String,
    title: Option<String>,
    mime_type: Option<String>,
}

/// Video from polling
#[derive(Debug, Deserialize)]
struct PollingVideo {
    file_id: String,
    mime_type: Option<String>,
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

/// Media file reference extracted from a polling message
#[derive(Debug)]
pub struct MediaFileRef {
    /// Telegram file_id for download
    pub file_id: String,
    /// MIME type (best guess)
    pub mime_type: String,
    /// Original filename if available
    pub filename: Option<String>,
}

/// Extract media file references from a polling message
fn extract_media_refs(msg: &PollingMessage) -> Vec<MediaFileRef> {
    let mut refs = Vec::new();

    // Photo: pick largest size (last in array)
    if let Some(photos) = &msg.photo {
        if let Some(largest) = photos.last() {
            refs.push(MediaFileRef {
                file_id: largest.file_id.clone(),
                mime_type: "image/jpeg".to_string(),
                filename: None,
            });
        }
    }

    if let Some(doc) = &msg.document {
        refs.push(MediaFileRef {
            file_id: doc.file_id.clone(),
            mime_type: doc.mime_type.clone().unwrap_or_else(|| "application/octet-stream".to_string()),
            filename: doc.file_name.clone(),
        });
    }

    if let Some(voice) = &msg.voice {
        refs.push(MediaFileRef {
            file_id: voice.file_id.clone(),
            mime_type: voice.mime_type.clone().unwrap_or_else(|| "audio/ogg".to_string()),
            filename: None,
        });
    }

    if let Some(audio) = &msg.audio {
        refs.push(MediaFileRef {
            file_id: audio.file_id.clone(),
            mime_type: audio.mime_type.clone().unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: audio.title.clone(),
        });
    }

    if let Some(video) = &msg.video {
        refs.push(MediaFileRef {
            file_id: video.file_id.clone(),
            mime_type: video.mime_type.clone().unwrap_or_else(|| "video/mp4".to_string()),
            filename: None,
        });
    }

    refs
}

/// Convert a polling update into an `IncomingMessage`
fn update_to_incoming(update: &PollingUpdate) -> Option<IncomingMessage> {
    let msg = update.message.as_ref()?;

    let has_media = msg.photo.is_some()
        || msg.document.is_some()
        || msg.voice.is_some()
        || msg.audio.is_some()
        || msg.video.is_some();

    let text = msg.text.clone().or_else(|| msg.caption.clone());

    // Skip messages with no text and no media
    if text.is_none() && !has_media {
        return None;
    }

    // Skip bot messages
    if msg.from.as_ref().is_some_and(|u| u.is_bot) {
        return None;
    }

    // Build content with media annotations as fallback text
    let content = if has_media {
        let mut parts = Vec::new();
        if let Some(ref t) = text {
            parts.push(t.clone());
        }
        if msg.photo.is_some() {
            parts.push("[Photo attached]".to_string());
        }
        if let Some(doc) = &msg.document {
            parts.push(format!(
                "[Document: {}]",
                doc.file_name.as_deref().unwrap_or("file")
            ));
        }
        if msg.audio.is_some() {
            parts.push("[Audio attached]".to_string());
        }
        if msg.video.is_some() {
            parts.push("[Video attached]".to_string());
        }
        if msg.voice.is_some() {
            parts.push("[Voice message]".to_string());
        }
        parts.join("\n")
    } else {
        text.unwrap_or_default()
    };

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
        content,
        is_dm,
        reply_to: None,
        attachments: vec![],
        thread_id: msg.message_thread_id.map(|id| id.to_string()),
        callback_data: None,
    })
}

/// Extract media refs from a polling update (for download by caller)
pub fn extract_update_media_refs(update: &PollingUpdate) -> Vec<MediaFileRef> {
    update
        .message
        .as_ref()
        .map(extract_media_refs)
        .unwrap_or_default()
}

/// Check if a polling message should be skipped due to mention gating.
///
/// Returns `true` if the message is in a group and the bot is not mentioned.
pub fn should_skip_group_message(
    msg: &IncomingMessage,
    chat_type: &str,
    has_reply: bool,
    config: &crate::config::TelegramConfig,
) -> bool {
    let is_group = chat_type == "group" || chat_type == "supergroup";
    if !is_group || !config.require_mention_in_groups {
        return false;
    }

    let mentioned = config
        .bot_username
        .as_ref()
        .is_some_and(|username| msg.content.contains(&format!("@{username}")))
        || has_reply;

    !mentioned
}
