//! Discord channel adapter using serenity

use std::sync::Arc;

use async_trait::async_trait;
use serenity::Client;
use serenity::all::{
    ChannelId, Context, CreateEmbed, CreateMessage, EventHandler, GatewayIntents, Message,
    MessageId, ReactionType, Ready,
};
use tokio::sync::{Mutex, mpsc};

use super::{Attachment, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Discord channel adapter
pub struct DiscordChannel {
    token: String,
    #[allow(dead_code)]
    client: Option<Client>,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    http: Option<Arc<serenity::http::Http>>,
    connected: bool,
}

impl DiscordChannel {
    /// Create a new Discord channel adapter
    ///
    /// # Arguments
    ///
    /// * `token` - Discord bot token
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: None,
            message_tx: None,
            http: None,
            connected: false,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(token: String) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            token,
            client: None,
            message_tx: Some(tx),
            http: None,
            connected: false,
        };
        (channel, rx)
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &'static str {
        "discord"
    }

    async fn connect(&mut self) -> Result<()> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let message_tx = self.message_tx.clone();

        let handler = DiscordHandler {
            message_tx: Arc::new(Mutex::new(message_tx)),
        };

        let client = Client::builder(&self.token, intents)
            .event_handler(handler)
            .await
            .map_err(|e| Error::Channel(format!("Discord client error: {e}")))?;

        self.http = Some(client.http.clone());

        // Spawn the client in a background task
        let mut client_runner = client;
        tokio::spawn(async move {
            if let Err(e) = client_runner.start().await {
                tracing::error!(error = %e, "Discord client error");
            }
        });

        self.connected = true;
        tracing::info!("Discord channel connected");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        // Client will be dropped when the task completes
        tracing::info!("Discord channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| Error::Channel("Discord not connected".to_string()))?;

        let channel_id: u64 = message
            .channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid channel ID".to_string()))?;

        let channel = ChannelId::new(channel_id);

        // Build message with optional embed for code blocks
        let builder = if message.has_code_blocks() {
            // Extract code blocks and create embeds
            let code_blocks = message.extract_code_blocks();
            let mut builder = CreateMessage::new();

            // Add non-code content as regular message
            let plain_content = remove_code_blocks(&message.content);
            if !plain_content.trim().is_empty() {
                builder = builder.content(plain_content.trim());
            }

            // Add code blocks as embeds (Discord has better rendering)
            for (lang, code) in code_blocks {
                let title = if lang.is_empty() {
                    "Code".to_string()
                } else {
                    lang.clone()
                };

                // Discord embeds have a 4096 char limit for description
                let truncated = if code.len() > 4000 {
                    format!("{}...\n(truncated)", &code[..4000])
                } else {
                    code
                };

                let embed = CreateEmbed::new()
                    .title(title)
                    .description(format!("```{lang}\n{truncated}\n```"))
                    .color(0x002B_2D31); // Discord dark theme color

                builder = builder.add_embed(embed);
            }

            builder
        } else {
            CreateMessage::new().content(&message.content)
        };

        channel
            .send_message(http, builder)
            .await
            .map_err(|e| Error::Channel(format!("Discord send error: {e}")))?;

        tracing::debug!(channel_id = %message.channel_id, "Discord message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, channel_id: &str) -> Result<()> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| Error::Channel("Discord not connected".to_string()))?;

        let id: u64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid channel ID".to_string()))?;

        let channel = ChannelId::new(id);

        channel
            .broadcast_typing(http)
            .await
            .map_err(|e| Error::Channel(format!("Discord typing error: {e}")))?;

        tracing::debug!(channel_id, "Discord typing indicator sent");
        Ok(())
    }

    async fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<()> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| Error::Channel("Discord not connected".to_string()))?;

        let chan_id: u64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid channel ID".to_string()))?;

        let msg_id: u64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        let reaction = parse_discord_emoji(emoji);

        http.create_reaction(ChannelId::new(chan_id), MessageId::new(msg_id), &reaction)
            .await
            .map_err(|e| Error::Channel(format!("Discord reaction error: {e}")))?;

        tracing::debug!(channel_id, message_id, emoji, "Discord reaction added");
        Ok(())
    }

    async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| Error::Channel("Discord not connected".to_string()))?;

        let chan_id: u64 = channel_id
            .parse()
            .map_err(|_| Error::Channel("Invalid channel ID".to_string()))?;

        let msg_id: u64 = message_id
            .parse()
            .map_err(|_| Error::Channel("Invalid message ID".to_string()))?;

        let reaction = parse_discord_emoji(emoji);

        http.delete_reaction_me(ChannelId::new(chan_id), MessageId::new(msg_id), &reaction)
            .await
            .map_err(|e| Error::Channel(format!("Discord remove reaction error: {e}")))?;

        tracing::debug!(channel_id, message_id, emoji, "Discord reaction removed");
        Ok(())
    }
}

/// Parse an emoji string into a Discord `ReactionType`
///
/// Handles both Unicode emoji (e.g., "👀") and custom Discord emoji (e.g., "<:name:123>")
fn parse_discord_emoji(emoji: &str) -> ReactionType {
    // Check for custom emoji format: <:name:id> or <a:name:id> (animated)
    if emoji.starts_with('<') && emoji.ends_with('>') {
        let inner = &emoji[1..emoji.len() - 1];
        let parts: Vec<&str> = inner.split(':').collect();
        if parts.len() == 3 {
            let animated = parts[0] == "a";
            let name = parts[1].to_string();
            if let Ok(id) = parts[2].parse::<u64>() {
                return ReactionType::Custom {
                    animated,
                    id: serenity::all::EmojiId::new(id),
                    name: Some(name),
                };
            }
        }
    }

    // Default to Unicode emoji
    ReactionType::Unicode(emoji.to_string())
}

/// Discord event handler
struct DiscordHandler {
    message_tx: Arc<Mutex<Option<mpsc::Sender<IncomingMessage>>>>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(user = %ready.user.name, "Discord bot ready");
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        let is_dm = msg.guild_id.is_none();
        let is_mention = msg.mentions_me(&ctx).await.unwrap_or(false);

        // Only respond to DMs or mentions
        if !is_dm && !is_mention {
            return;
        }

        // Extract reply-to if this is a reply
        let reply_to = msg
            .referenced_message
            .as_ref()
            .map(|r| r.id.to_string());

        // Extract attachments from Discord message
        let attachments = msg
            .attachments
            .iter()
            .map(|att| {
                let mime_type = att
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                Attachment::from_url(att.url.clone(), mime_type, Some(att.filename.clone()))
            })
            .collect();

        let incoming = IncomingMessage {
            id: msg.id.to_string(),
            channel_id: msg.channel_id.to_string(),
            sender_id: msg.author.id.to_string(),
            sender_name: msg.author.name.clone(),
            content: msg.content.clone(),
            is_dm,
            reply_to,
            attachments,
        };

        if let Some(tx) = self.message_tx.lock().await.as_ref() {
            if let Err(e) = tx.send(incoming).await {
                tracing::warn!(error = %e, "Failed to forward Discord message");
            }
        }

        tracing::debug!(
            author = %msg.author.name,
            content = %msg.content,
            is_dm,
            "Discord message received"
        );
    }
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
