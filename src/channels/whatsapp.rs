//! `WhatsApp` channel adapter
//!
//! Uses `WhatsApp` Business API for messaging.
//! For receiving messages, use the `WhatsApp` Webhooks API with an endpoint.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{Attachment, AttachmentKind, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// `WhatsApp` channel adapter
pub struct WhatsAppChannel {
    /// `WhatsApp` Business API access token
    access_token: String,
    /// Phone number ID for sending messages
    phone_number_id: String,
    client: Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
}

impl WhatsAppChannel {
    /// Create a new `WhatsApp` channel adapter
    ///
    /// # Arguments
    ///
    /// * `access_token` - `WhatsApp` Business API access token
    /// * `phone_number_id` - Phone number ID registered with `WhatsApp` Business
    #[must_use]
    pub fn new(access_token: String, phone_number_id: String) -> Self {
        Self {
            access_token,
            phone_number_id,
            client: Client::new(),
            message_tx: None,
            connected: false,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(
        access_token: String,
        phone_number_id: String,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            access_token,
            phone_number_id,
            client: Client::new(),
            message_tx: Some(tx),
            connected: false,
        };
        (channel, rx)
    }

    /// Process an incoming `WhatsApp` webhook event
    ///
    /// Call this from your webhook handler when receiving events
    ///
    /// # Errors
    ///
    /// Returns error if message forwarding fails
    pub async fn handle_webhook(&self, payload: &WhatsAppWebhook) -> Result<()> {
        for entry in &payload.entry {
            for change in &entry.changes {
                if let Some(ref messages) = change.value.messages {
                    for msg in messages {
                        // Get text content (direct or from caption)
                        let mut content = msg
                            .text
                            .as_ref()
                            .map(|t| t.body.clone())
                            .unwrap_or_default();

                        // Build attachments from media
                        let mut attachments = Vec::new();

                        if let Some(image) = &msg.image {
                            let mime = image.mime_type.clone().unwrap_or_else(|| "image/jpeg".to_string());
                            if let Some(caption) = &image.caption {
                                if content.is_empty() {
                                    content = caption.clone();
                                }
                            }
                            attachments.push(Attachment {
                                kind: AttachmentKind::Image,
                                url: Some(format!("whatsapp://media/{}", image.id)),
                                data: None,
                                mime_type: mime,
                                filename: None,
                            });
                        }

                        if let Some(doc) = &msg.document {
                            let mime = doc.mime_type.clone().unwrap_or_else(|| "application/octet-stream".to_string());
                            if let Some(caption) = &doc.caption {
                                if content.is_empty() {
                                    content = caption.clone();
                                }
                            }
                            attachments.push(Attachment {
                                kind: AttachmentKind::from_mime(&mime),
                                url: Some(format!("whatsapp://media/{}", doc.id)),
                                data: None,
                                mime_type: mime,
                                filename: doc.filename.clone(),
                            });
                        }

                        if let Some(audio) = &msg.audio {
                            let mime = audio.mime_type.clone().unwrap_or_else(|| "audio/ogg".to_string());
                            attachments.push(Attachment {
                                kind: AttachmentKind::Audio,
                                url: Some(format!("whatsapp://media/{}", audio.id)),
                                data: None,
                                mime_type: mime,
                                filename: None,
                            });
                        }

                        if let Some(video) = &msg.video {
                            let mime = video.mime_type.clone().unwrap_or_else(|| "video/mp4".to_string());
                            if let Some(caption) = &video.caption {
                                if content.is_empty() {
                                    content = caption.clone();
                                }
                            }
                            attachments.push(Attachment {
                                kind: AttachmentKind::Video,
                                url: Some(format!("whatsapp://media/{}", video.id)),
                                data: None,
                                mime_type: mime,
                                filename: None,
                            });
                        }

                        // Skip if no content and no attachments
                        if content.is_empty() && attachments.is_empty() {
                            continue;
                        }

                        let incoming = IncomingMessage {
                            id: msg.id.clone(),
                            channel_id: msg.from.clone(),
                            sender_id: msg.from.clone(),
                            sender_name: msg.from.clone(),
                            content,
                            is_dm: true,
                            reply_to: msg.context.as_ref().map(|c| c.id.clone()),
                            attachments,
                            thread_id: None,
                            callback_data: None,
                        };

                        if let Some(tx) = &self.message_tx {
                            tx.send(incoming).await.map_err(|e| {
                                Error::Channel(format!("Failed to forward message: {e}"))
                            })?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Send a text message to a `WhatsApp` number
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_text(&self, to: &str, text: &str, reply_to: Option<&str>) -> Result<()> {
        let url = format!(
            "https://graph.facebook.com/v18.0/{}/messages",
            self.phone_number_id
        );

        // Check if message contains code blocks (disable preview for code)
        let has_code = text.contains("```");

        let mut body = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": to,
            "type": "text",
            "text": {
                "body": text,
                "preview_url": !has_code
            }
        });

        // Add reply context if replying to a message
        if let Some(message_id) = reply_to {
            body["context"] = serde_json::json!({
                "message_id": message_id
            });
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("WhatsApp API error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "WhatsApp API error: {status} - {body}"
            )));
        }

        tracing::debug!(to, "WhatsApp message sent");
        Ok(())
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &'static str {
        "whatsapp"
    }

    async fn connect(&mut self) -> Result<()> {
        // WhatsApp uses webhooks; "connect" validates the configuration
        if self.access_token.is_empty() {
            return Err(Error::Channel(
                "WhatsApp access token required".to_string(),
            ));
        }
        if self.phone_number_id.is_empty() {
            return Err(Error::Channel(
                "WhatsApp phone number ID required".to_string(),
            ));
        }

        self.connected = true;
        tracing::info!("WhatsApp channel connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("WhatsApp channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        self.send_text(&message.channel_id, &message.content, message.reply_to.as_deref()).await
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// `WhatsApp` webhook payload from Cloud API
#[derive(Debug, Deserialize)]
pub struct WhatsAppWebhook {
    /// Webhook entries
    pub entry: Vec<WhatsAppWebhookEntry>,
}

/// `WhatsApp` webhook entry
#[derive(Debug, Deserialize)]
pub struct WhatsAppWebhookEntry {
    /// Changes in this entry
    pub changes: Vec<WhatsAppWebhookChange>,
}

/// `WhatsApp` webhook change
#[derive(Debug, Deserialize)]
pub struct WhatsAppWebhookChange {
    /// The change value
    pub value: WhatsAppWebhookValue,
}

/// `WhatsApp` webhook value containing messages
#[derive(Debug, Deserialize)]
pub struct WhatsAppWebhookValue {
    /// Incoming messages (if any)
    pub messages: Option<Vec<WhatsAppMessage>>,
}

/// `WhatsApp` message
#[derive(Debug, Deserialize)]
pub struct WhatsAppMessage {
    /// Sender phone number
    pub from: String,
    /// Message ID
    pub id: String,
    /// Message timestamp
    pub timestamp: String,
    /// Message type
    #[serde(rename = "type")]
    pub message_type: String,
    /// Text content (for text messages)
    pub text: Option<WhatsAppTextContent>,
    /// Image content
    pub image: Option<WhatsAppMedia>,
    /// Document content
    pub document: Option<WhatsAppDocument>,
    /// Audio content
    pub audio: Option<WhatsAppMedia>,
    /// Video content
    pub video: Option<WhatsAppMedia>,
    /// Context for reply messages
    pub context: Option<WhatsAppContext>,
}

/// `WhatsApp` media object (image, audio, video)
#[derive(Debug, Deserialize)]
pub struct WhatsAppMedia {
    /// Media ID (use to fetch URL)
    pub id: String,
    /// MIME type
    pub mime_type: Option<String>,
    /// Caption
    pub caption: Option<String>,
}

/// `WhatsApp` document media
#[derive(Debug, Deserialize)]
pub struct WhatsAppDocument {
    /// Media ID (use to fetch URL)
    pub id: String,
    /// MIME type
    pub mime_type: Option<String>,
    /// Filename
    pub filename: Option<String>,
    /// Caption
    pub caption: Option<String>,
}

/// `WhatsApp` message context (for replies)
#[derive(Debug, Deserialize)]
pub struct WhatsAppContext {
    /// ID of the message being replied to
    pub id: String,
}

/// `WhatsApp` text message content
#[derive(Debug, Deserialize)]
pub struct WhatsAppTextContent {
    /// Message body
    pub body: String,
}
