//! Telegram channel adapter
//!
//! Uses webhooks for receiving messages and Bot API for sending

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{Channel, OutgoingMessage};
use crate::{Error, Result};

/// Telegram Bot API base URL
const API_BASE: &str = "https://api.telegram.org/bot";

/// Telegram channel adapter
#[derive(Clone)]
pub struct TelegramChannel {
    token: String,
    client: Client,
    connected: bool,
}

impl TelegramChannel {
    /// Create a new Telegram channel adapter
    #[must_use]
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: Client::new(),
            connected: false,
        }
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
