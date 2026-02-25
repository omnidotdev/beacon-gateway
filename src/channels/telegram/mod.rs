//! Telegram channel adapter
//!
//! Uses webhooks for receiving messages and Bot API for sending

mod api;
pub mod chunking;
pub mod dedup;
pub mod html;
pub mod polling;
pub mod rate_limiter;
pub mod retry;
pub mod types;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::mpsc;

use super::{Channel, ChannelCapability, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

pub use polling::extract_update_media_refs;
pub use rate_limiter::TelegramRateLimiter;
pub use dedup::UpdateDedup;
pub use types::{BotCommand, MediaFileRef};

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
            ChannelCapability::Stickers,
        ]
    }

    async fn connect(&mut self) -> Result<()> {
        self.get_me().await?;
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

        self.send_chat_action(chat_id, "typing").await?;
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

        self.set_message_reaction(chat_id, msg_id, emoji).await
    }

    async fn remove_reaction(&self, channel_id: &str, message_id: &str, _emoji: &str) -> Result<()> {
        let chat_id: i64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid chat ID".to_string()))?;
        let msg_id: i64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        self.clear_message_reaction(chat_id, msg_id).await
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

/// A running Telegram bot account
#[derive(Clone)]
pub struct TelegramAccount {
    /// Account identifier (e.g., "default", "support", "alerts")
    pub id: String,
    /// Telegram channel adapter for this account
    pub channel: TelegramChannel,
    /// Per-account config overrides
    pub config: crate::config::TelegramAccountConfig,
}

/// Registry managing multiple Telegram bot accounts
///
/// Supports single-bot backward compat (one "default" account) and multi-bot
/// mode with named accounts. Each account has its own channel and config.
#[derive(Clone)]
pub struct TelegramAccountRegistry {
    accounts: HashMap<String, TelegramAccount>,
    default_id: String,
}

impl TelegramAccountRegistry {
    /// Create a registry with a single default account
    #[must_use]
    pub fn single(channel: TelegramChannel, config: crate::config::TelegramAccountConfig) -> Self {
        let mut accounts = HashMap::new();
        accounts.insert(
            "default".to_string(),
            TelegramAccount {
                id: "default".to_string(),
                channel,
                config,
            },
        );
        Self {
            accounts,
            default_id: "default".to_string(),
        }
    }

    /// Create a registry from a map of named accounts
    #[must_use]
    pub fn new(accounts: HashMap<String, TelegramAccount>, default_id: String) -> Self {
        Self {
            accounts,
            default_id,
        }
    }

    /// Get the default account's channel
    #[must_use]
    pub fn default_channel(&self) -> Option<&TelegramChannel> {
        self.accounts.get(&self.default_id).map(|a| &a.channel)
    }

    /// Get the default account
    #[must_use]
    pub fn default_account(&self) -> Option<&TelegramAccount> {
        self.accounts.get(&self.default_id)
    }

    /// Get the default account ID
    #[must_use]
    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    /// Look up an account by ID
    #[must_use]
    pub fn get(&self, account_id: &str) -> Option<&TelegramAccount> {
        self.accounts.get(account_id)
    }

    /// Iterate over all accounts
    pub fn iter(&self) -> impl Iterator<Item = (&String, &TelegramAccount)> {
        self.accounts.iter()
    }

    /// Number of registered accounts
    #[must_use]
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// Whether the registry is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }
}

/// Check if a message should be skipped due to mention gating.
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
