//! Messaging channel adapters
//!
//! Each channel implements the `Channel` trait to provide unified messaging.

mod discord;
mod google_chat;
mod imessage;
mod matrix;
mod signal;
mod slack;
mod teams;
mod telegram;
mod whatsapp;

use async_trait::async_trait;

pub use discord::DiscordChannel;
pub use google_chat::{GoogleChatChannel, GoogleChatEvent};
pub use imessage::{IMessageChannel, IMessageChat, IMessageMessage};
pub use matrix::MatrixChannel;
pub use signal::{SignalChannel, SignalMessage};
pub use slack::{SlackChannel, SlackEvent};
pub use teams::{TeamsActivity, TeamsChannel};
pub use telegram::TelegramChannel;
pub use whatsapp::{WhatsAppChannel, WhatsAppWebhook};

use crate::Result;

/// Feature a channel adapter may support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelCapability {
    /// Real-time token streaming
    Streaming,
    /// Message reactions (emoji)
    Reactions,
    /// Inline keyboard / button attachments
    InlineKeyboards,
    /// Sending media (images, audio, video, files)
    MediaSend,
    /// Editing previously sent messages
    MessageEdit,
    /// Deleting previously sent messages
    MessageDelete,
    /// Voice-to-text transcription
    VoiceTranscribe,
    /// Forum / topic threads
    ForumTopics,
    /// Sticker messages
    Stickers,
}

/// Type of attachment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// Image file (JPEG, PNG, GIF, etc.)
    Image,
    /// Audio file (MP3, WAV, OGG, etc.)
    Audio,
    /// Video file (MP4, MOV, etc.)
    Video,
    /// Generic file
    File,
}

/// An attachment on an incoming message
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Type of attachment
    pub kind: AttachmentKind,

    /// URL to download the attachment (if available)
    pub url: Option<String>,

    /// Raw attachment data (if available)
    pub data: Option<Vec<u8>>,

    /// MIME type
    pub mime_type: String,

    /// Original filename
    pub filename: Option<String>,
}

impl Attachment {
    /// Create an attachment from a URL
    #[must_use]
    pub fn from_url(url: String, mime_type: String, filename: Option<String>) -> Self {
        Self {
            kind: AttachmentKind::from_mime(&mime_type),
            url: Some(url),
            data: None,
            mime_type,
            filename,
        }
    }

    /// Create an attachment from inline data
    #[must_use]
    pub fn from_data(data: Vec<u8>, mime_type: String, filename: Option<String>) -> Self {
        Self {
            kind: AttachmentKind::from_mime(&mime_type),
            url: None,
            data: Some(data),
            mime_type,
            filename,
        }
    }
}

impl AttachmentKind {
    /// Determine attachment kind from MIME type
    #[must_use]
    pub fn from_mime(mime_type: &str) -> Self {
        let lower = mime_type.to_lowercase();
        if lower.starts_with("image/") {
            Self::Image
        } else if lower.starts_with("audio/") {
            Self::Audio
        } else if lower.starts_with("video/") {
            Self::Video
        } else {
            Self::File
        }
    }
}

/// A message from a channel
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// Message identifier (platform-specific)
    pub id: String,

    /// Channel identifier
    pub channel_id: String,

    /// Sender identifier
    pub sender_id: String,

    /// Sender display name
    pub sender_name: String,

    /// Message content
    pub content: String,

    /// Whether this is a direct message
    pub is_dm: bool,

    /// Message this is replying to (if any)
    pub reply_to: Option<String>,

    /// Attachments on the message
    pub attachments: Vec<Attachment>,
}

/// A message to send to a channel
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    /// Channel identifier
    pub channel_id: String,

    /// Message content (plain text, may contain markdown)
    pub content: String,

    /// Optional reply-to message ID
    pub reply_to: Option<String>,
}

impl OutgoingMessage {
    /// Create a simple `text` message
    #[must_use]
    pub fn text(channel_id: String, content: String) -> Self {
        Self {
            channel_id,
            content,
            reply_to: None,
        }
    }

    /// Create a `reply` message
    #[must_use]
    pub fn reply(channel_id: String, content: String, reply_to: String) -> Self {
        Self {
            channel_id,
            content,
            reply_to: Some(reply_to),
        }
    }

    /// Check if content contains code blocks
    #[must_use]
    pub fn has_code_blocks(&self) -> bool {
        self.content.contains("```")
    }

    /// Check if content contains markdown formatting
    #[must_use]
    pub fn has_markdown(&self) -> bool {
        self.content.contains("**")
            || self.content.contains("__")
            || self.content.contains('`')
            || self.content.contains("- ")
            || self.content.contains("* ")
            || self.content.contains("# ")
    }

    /// Extract code blocks from content
    ///
    /// Returns vec of (language, code) tuples
    #[must_use]
    pub fn extract_code_blocks(&self) -> Vec<(String, String)> {
        let mut blocks = Vec::new();
        let mut in_block = false;
        let mut current_lang = String::new();
        let mut current_code = String::new();

        for line in self.content.lines() {
            if line.starts_with("```") {
                if in_block {
                    // End of block
                    blocks.push((current_lang.clone(), current_code.trim().to_string()));
                    current_lang.clear();
                    current_code.clear();
                    in_block = false;
                } else {
                    // Start of block
                    current_lang = line.trim_start_matches('`').to_string();
                    in_block = true;
                }
            } else if in_block {
                if !current_code.is_empty() {
                    current_code.push('\n');
                }
                current_code.push_str(line);
            }
        }

        blocks
    }
}

/// Trait for messaging channel adapters
#[async_trait]
pub trait Channel: Send + Sync {
    /// Get the channel name
    fn name(&self) -> &'static str;

    /// Declare which capabilities this channel supports
    fn capabilities(&self) -> &'static [ChannelCapability] {
        &[]
    }

    /// Connect to the channel
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect from the channel
    async fn disconnect(&mut self) -> Result<()>;

    /// Send a message
    async fn send(&self, message: OutgoingMessage) -> Result<()>;

    /// Check if connected
    fn is_connected(&self) -> bool;

    /// Send typing indicator to show the bot is processing
    ///
    /// Default implementation is a no-op for channels that don't support typing
    async fn send_typing(&self, _channel_id: &str) -> Result<()> {
        Ok(())
    }

    /// Add a reaction to a message
    ///
    /// Default implementation is a no-op for channels that don't support reactions
    async fn add_reaction(&self, _channel_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
        Ok(())
    }

    /// Remove a reaction from a message
    ///
    /// Default implementation is a no-op for channels that don't support reactions
    async fn remove_reaction(
        &self,
        _channel_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<()> {
        Ok(())
    }
}

/// Channel registry - manages multiple channel adapters
pub struct ChannelRegistry {
    channels: Vec<Box<dyn Channel>>,
}

impl ChannelRegistry {
    /// Create a new channel registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// Register a channel adapter
    pub fn register(&mut self, channel: Box<dyn Channel>) {
        self.channels.push(channel);
    }

    /// Connect all registered channels
    ///
    /// # Errors
    ///
    /// Returns error if any channel fails to connect
    pub async fn connect_all(&mut self) -> Result<()> {
        for channel in &mut self.channels {
            tracing::info!(channel = channel.name(), "connecting channel");
            channel.connect().await?;
        }
        Ok(())
    }

    /// Disconnect all channels
    pub async fn disconnect_all(&mut self) {
        for channel in &mut self.channels {
            if let Err(e) = channel.disconnect().await {
                tracing::warn!(channel = channel.name(), error = %e, "disconnect failed");
            }
        }
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
