//! Signal channel adapter
//!
//! Uses Signal CLI or signal-cli-rest-api for messaging

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use tokio::sync::mpsc;

use super::{Attachment, AttachmentKind, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Signal channel adapter
///
/// Requires signal-cli-rest-api running locally or remotely
/// See: <https://github.com/bbernhard/signal-cli-rest-api>
pub struct SignalChannel {
    /// Base URL for signal-cli-rest-api
    api_url: String,
    /// Sender phone number (registered with Signal)
    sender_number: String,
    client: Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    connected: bool,
}

impl SignalChannel {
    /// Create a new Signal channel adapter
    ///
    /// # Arguments
    ///
    /// * `api_url` - Base URL for signal-cli-rest-api (e.g., `http://localhost:8080`)
    /// * `sender_number` - Phone number registered with Signal (e.g., "+1234567890")
    #[must_use]
    pub fn new(api_url: String, sender_number: String) -> Self {
        Self {
            api_url,
            sender_number,
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
        api_url: String,
        sender_number: String,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            api_url,
            sender_number,
            client: Client::new(),
            message_tx: Some(tx),
            connected: false,
        };
        (channel, rx)
    }

    /// Process an incoming Signal message
    ///
    /// Call this from your webhook handler or polling loop when receiving messages
    ///
    /// # Errors
    ///
    /// Returns error if message forwarding fails
    pub async fn handle_incoming(&self, message: &SignalMessage) -> Result<()> {
        // Accept messages with text or attachments
        let has_content = message.message.as_ref().is_some_and(|t| !t.is_empty());
        let has_attachments = message.attachments.as_ref().is_some_and(|a| !a.is_empty());

        if !has_content && !has_attachments {
            return Ok(());
        }

        let content = message.message.clone().unwrap_or_default();

        let sender = message
            .source_number
            .clone()
            .or_else(|| message.source_uuid.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Use timestamp as unique ID
        let id = message
            .timestamp
            .map(|ts| ts.to_string())
            .unwrap_or_default();

        // Parse attachments (Signal provides base64 data inline)
        let attachments = message
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .map(|att| {
                        let mime_type = att
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string());
                        let filename = att.filename.clone();

                        // Decode base64 data if present
                        if let Some(b64) = &att.data {
                            if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(b64) {
                                return Attachment::from_data(data, mime_type, filename);
                            }
                        }

                        // No data available, include metadata only
                        Attachment {
                            kind: AttachmentKind::from_mime(&mime_type),
                            url: None,
                            data: None,
                            mime_type,
                            filename,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let incoming = IncomingMessage {
            id,
            channel_id: sender.clone(),
            sender_id: sender.clone(),
            sender_name: sender,
            content,
            is_dm: true,
            reply_to: None,
            attachments,
            thread_id: None,
            callback_data: None,
        };

        if let Some(tx) = &self.message_tx {
            tx.send(incoming).await.map_err(|e| {
                Error::Channel(format!("Failed to forward message: {e}"))
            })?;
        }

        Ok(())
    }

    /// Send a text message to a Signal number
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_text(&self, to: &str, text: &str) -> Result<()> {
        let url = format!("{}/v2/send", self.api_url);

        let body = serde_json::json!({
            "message": text,
            "number": self.sender_number,
            "recipients": [to]
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Signal API error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Signal API error: {status} - {body}"
            )));
        }

        tracing::debug!(to, "Signal message sent");
        Ok(())
    }

    /// Receive pending messages
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn receive(&self) -> Result<Vec<SignalMessage>> {
        let url = format!("{}/v1/receive/{}", self.api_url, self.sender_number);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Signal receive error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Signal receive error: {status} - {body}"
            )));
        }

        let messages: Vec<SignalMessage> = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Signal parse error: {e}")))?;

        Ok(messages)
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &'static str {
        "signal"
    }

    async fn connect(&mut self) -> Result<()> {
        // Signal uses polling or webhooks; "connect" validates the configuration
        if self.api_url.is_empty() {
            return Err(Error::Channel("Signal API URL required".to_string()));
        }
        if self.sender_number.is_empty() {
            return Err(Error::Channel(
                "Signal sender number required".to_string(),
            ));
        }

        // Verify API is reachable
        let url = format!("{}/v1/about", self.api_url);
        let response = self.client.get(&url).send().await;

        if let Err(e) = response {
            return Err(Error::Channel(format!(
                "Signal API not reachable: {e}"
            )));
        }

        self.connected = true;
        tracing::info!("Signal channel connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Signal channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        self.send_text(&message.channel_id, &message.content).await
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// A received Signal message
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalMessage {
    /// Source phone number
    pub source_number: Option<String>,
    /// Source UUID
    pub source_uuid: Option<String>,
    /// Message text
    pub message: Option<String>,
    /// Timestamp
    pub timestamp: Option<i64>,
    /// Attachments
    pub attachments: Option<Vec<SignalAttachment>>,
}

/// Signal attachment from signal-cli-rest-api
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalAttachment {
    /// MIME type
    pub content_type: Option<String>,
    /// Filename
    pub filename: Option<String>,
    /// Base64 encoded data (if fetched)
    pub data: Option<String>,
    /// File size
    pub size: Option<u64>,
}
