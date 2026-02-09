//! Google Chat channel adapter using Chat API
//!
//! Uses service account authentication and webhooks for receiving messages

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use super::{Attachment, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

const GOOGLE_CHAT_API_URL: &str = "https://chat.googleapis.com/v1";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const TOKEN_SCOPE: &str = "https://www.googleapis.com/auth/chat.bot";

/// Google Chat channel adapter
pub struct GoogleChatChannel {
    service_account_path: PathBuf,
    client: reqwest::Client,
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
    access_token: Arc<Mutex<Option<TokenInfo>>>,
    connected: bool,
}

/// Cached token info
struct TokenInfo {
    access_token: String,
    expires_at: u64,
}

/// Service account JSON structure
#[derive(Debug, Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
    #[allow(dead_code)]
    project_id: Option<String>,
}

/// JWT header (used internally by jsonwebtoken crate)
#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct JwtHeader {
    alg: &'static str,
    typ: &'static str,
}

/// JWT claims for Google OAuth
#[derive(Debug, Serialize)]
struct JwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    exp: u64,
    iat: u64,
}

/// Token response from Google
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

/// Google Chat message (simple text)
#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<ThreadRef<'a>>,
}

/// Google Chat message with cards
#[derive(Debug, Serialize)]
struct ChatMessageWithCards<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(rename = "cardsV2")]
    cards_v2: Vec<CardV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<ThreadRef<'a>>,
}

/// Card V2 wrapper
#[derive(Debug, Serialize)]
struct CardV2 {
    #[serde(rename = "cardId")]
    card_id: String,
    card: Card,
}

/// Card structure
#[derive(Debug, Serialize)]
struct Card {
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<CardHeader>,
    sections: Vec<CardSection>,
}

/// Card header
#[derive(Debug, Serialize)]
struct CardHeader {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<String>,
}

/// Card section
#[derive(Debug, Serialize)]
struct CardSection {
    widgets: Vec<CardWidget>,
}

/// Card widget
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum CardWidget {
    TextParagraph { text: String },
}

/// Thread reference
#[derive(Debug, Serialize)]
struct ThreadRef<'a> {
    name: &'a str,
}

/// Google Chat event from webhook
#[derive(Debug, Deserialize)]
pub struct GoogleChatEvent {
    /// Event type (`MESSAGE`, `ADDED_TO_SPACE`, etc.)
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event time
    #[serde(rename = "eventTime")]
    pub event_time: Option<String>,
    /// Space info
    pub space: Option<SpaceInfo>,
    /// Message info (for MESSAGE events)
    pub message: Option<MessageInfo>,
    /// User who triggered the event
    pub user: Option<UserInfo>,
}

/// Space info
#[derive(Debug, Deserialize)]
pub struct SpaceInfo {
    /// Space resource name
    pub name: String,
    /// Space type (ROOM, DM, etc.)
    #[serde(rename = "type")]
    pub space_type: Option<String>,
    /// Display name
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// Message info
#[derive(Debug, Deserialize)]
pub struct MessageInfo {
    /// Message resource name
    pub name: String,
    /// Sender
    pub sender: Option<UserInfo>,
    /// Message text
    pub text: Option<String>,
    /// Thread info
    pub thread: Option<ThreadInfo>,
    /// Create time
    #[serde(rename = "createTime")]
    pub create_time: Option<String>,
    /// Attachments
    pub attachment: Option<Vec<GoogleChatAttachment>>,
}

/// Google Chat attachment
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleChatAttachment {
    /// Attachment resource name
    pub name: Option<String>,
    /// Content name (filename)
    pub content_name: Option<String>,
    /// MIME type
    pub content_type: Option<String>,
    /// Download URI
    pub attachment_data_ref: Option<AttachmentDataRef>,
    /// Drive data ref (for Drive attachments)
    pub drive_data_ref: Option<DriveDataRef>,
}

/// Attachment data reference
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDataRef {
    /// Resource name for downloading
    pub resource_name: String,
}

/// Drive data reference
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveDataRef {
    /// Drive file ID
    pub drive_file_id: String,
}

/// Thread info
#[derive(Debug, Deserialize)]
pub struct ThreadInfo {
    /// Thread resource name
    pub name: String,
}

/// User info
#[derive(Debug, Deserialize)]
pub struct UserInfo {
    /// User resource name
    pub name: String,
    /// Display name
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    /// User type (HUMAN, BOT)
    #[serde(rename = "type")]
    pub user_type: Option<String>,
}

impl GoogleChatChannel {
    /// Create a new Google Chat channel adapter
    ///
    /// # Arguments
    ///
    /// * `service_account_path` - Path to service account JSON file
    #[must_use]
    pub fn new(service_account_path: PathBuf) -> Self {
        Self {
            service_account_path,
            client: reqwest::Client::new(),
            message_tx: None,
            access_token: Arc::new(Mutex::new(None)),
            connected: false,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(
        service_account_path: PathBuf,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            service_account_path,
            client: reqwest::Client::new(),
            message_tx: Some(tx),
            access_token: Arc::new(Mutex::new(None)),
            connected: false,
        };
        (channel, rx)
    }

    /// Get sender for webhook handler
    #[must_use]
    pub fn sender(&self) -> Option<mpsc::Sender<IncomingMessage>> {
        self.message_tx.clone()
    }

    /// Load service account from file
    fn load_service_account(&self) -> Result<ServiceAccount> {
        let content = std::fs::read_to_string(&self.service_account_path)
            .map_err(|e| Error::Channel(format!("Failed to read service account: {e}")))?;

        serde_json::from_str(&content)
            .map_err(|e| Error::Channel(format!("Failed to parse service account: {e}")))
    }

    /// Create JWT for token request
    #[allow(clippy::unused_self)]
    fn create_jwt(&self, service_account: &ServiceAccount) -> Result<String> {
        use jsonwebtoken::{Algorithm, EncodingKey, Header};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let header = Header::new(Algorithm::RS256);
        let claims = JwtClaims {
            iss: &service_account.client_email,
            scope: TOKEN_SCOPE,
            aud: GOOGLE_TOKEN_URL,
            exp: now + 3600,
            iat: now,
        };

        let key = EncodingKey::from_rsa_pem(service_account.private_key.as_bytes())
            .map_err(|e| Error::Channel(format!("Invalid private key: {e}")))?;

        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| Error::Channel(format!("JWT encoding failed: {e}")))
    }

    /// Get or refresh access token
    async fn get_access_token(&self) -> Result<String> {
        // Check if we have a valid cached token
        {
            let token_guard = self.access_token.lock().await;
            if let Some(ref token_info) = *token_guard {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                // Return cached token if still valid (with 5 min buffer)
                if token_info.expires_at > now + 300 {
                    return Ok(token_info.access_token.clone());
                }
            }
        }

        // Need to refresh token
        let service_account = self.load_service_account()?;
        let jwt = self.create_jwt(&service_account)?;

        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Token request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Token request failed: {status} - {body}"
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Token parse error: {e}")))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let token_info = TokenInfo {
            access_token: token_response.access_token.clone(),
            expires_at: now + token_response.expires_in,
        };

        // Cache the token
        {
            let mut token_guard = self.access_token.lock().await;
            *token_guard = Some(token_info);
        }

        Ok(token_response.access_token)
    }

    /// Process an incoming Google Chat event (from webhook)
    ///
    /// # Errors
    ///
    /// Returns error if message forwarding fails
    pub async fn handle_event(&self, event: &GoogleChatEvent) -> Result<()> {
        // Only handle MESSAGE events
        if event.event_type != "MESSAGE" {
            tracing::debug!(event_type = %event.event_type, "Ignoring non-message event");
            return Ok(());
        }

        let message = event
            .message
            .as_ref()
            .ok_or_else(|| Error::Channel("MESSAGE event without message".to_string()))?;

        let space = event
            .space
            .as_ref()
            .ok_or_else(|| Error::Channel("Event without space".to_string()))?;

        // Skip bot messages
        if let Some(sender) = &message.sender {
            if sender.user_type.as_deref() == Some("BOT") {
                return Ok(());
            }
        }

        let sender = message.sender.as_ref();
        let sender_id = sender.map_or_else(String::new, |s| s.name.clone());
        let sender_name = sender
            .and_then(|s| s.display_name.clone())
            .unwrap_or_else(|| sender_id.clone());

        let is_dm = space.space_type.as_deref() == Some("DM");

        // Parse attachments
        let attachments = message
            .attachment
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .filter_map(|att| {
                        let mime_type = att
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string());
                        let filename = att.content_name.clone();

                        // Try to get download URL from attachment_data_ref
                        let url = att.attachment_data_ref.as_ref().map(|r| {
                            format!("{GOOGLE_CHAT_API_URL}/{}/media", r.resource_name)
                        });

                        // Only include if we have a URL
                        url.map(|u| Attachment::from_url(u, mime_type, filename))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let incoming = IncomingMessage {
            id: message.name.clone(),
            channel_id: space.name.clone(),
            sender_id,
            sender_name,
            content: message.text.clone().unwrap_or_default(),
            is_dm,
            reply_to: message.thread.as_ref().map(|t| t.name.clone()),
            attachments,
        };

        if let Some(tx) = &self.message_tx {
            tx.send(incoming)
                .await
                .map_err(|e| Error::Channel(format!("Failed to forward message: {e}")))?;
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for GoogleChatChannel {
    fn name(&self) -> &'static str {
        "google_chat"
    }

    async fn connect(&mut self) -> Result<()> {
        // Verify service account exists and is valid
        let service_account = self.load_service_account()?;

        tracing::info!(
            client_email = %service_account.client_email,
            "Google Chat service account loaded"
        );

        // Get initial access token to verify credentials
        let _token = self.get_access_token().await?;

        self.connected = true;
        tracing::info!("Google Chat channel connected");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Google Chat channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        let access_token = self.get_access_token().await?;

        let url = format!("{}/{}/messages", GOOGLE_CHAT_API_URL, message.channel_id);
        let thread = message.reply_to.as_ref().map(|name| ThreadRef { name });

        let response = if message.has_code_blocks() {
            // Build card for code blocks
            let code_blocks = message.extract_code_blocks();
            let plain_content = remove_code_blocks(&message.content);

            let mut widgets = Vec::new();

            // Add plain text if present
            if !plain_content.trim().is_empty() {
                widgets.push(CardWidget::TextParagraph {
                    text: plain_content.trim().to_string(),
                });
            }

            // Add code blocks as decorated text with monospace
            for (lang, code) in code_blocks {
                // Google Chat has size limits, truncate if needed
                let truncated = if code.len() > 4000 {
                    format!("{}...\n(truncated)", &code[..4000])
                } else {
                    code
                };

                // Add language header
                if !lang.is_empty() {
                    widgets.push(CardWidget::TextParagraph {
                        text: format!("<b>{lang}</b>"),
                    });
                }

                // Add code in monospace (using <pre> tag for monospace)
                widgets.push(CardWidget::TextParagraph {
                    text: format!("<pre>{}</pre>", html_escape(&truncated)),
                });
            }

            let card = CardV2 {
                card_id: format!("code_{}", uuid::Uuid::new_v4()),
                card: Card {
                    header: None,
                    sections: vec![CardSection { widgets }],
                },
            };

            let chat_message = ChatMessageWithCards {
                text: Some(&message.content), // Fallback text
                cards_v2: vec![card],
                thread,
            };

            self.client
                .post(&url)
                .bearer_auth(&access_token)
                .json(&chat_message)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Google Chat send failed: {e}")))?
        } else {
            let chat_message = ChatMessage {
                text: &message.content,
                thread,
            };

            self.client
                .post(&url)
                .bearer_auth(&access_token)
                .json(&chat_message)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Google Chat send failed: {e}")))?
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Google Chat send failed: {status} - {body}"
            )));
        }

        tracing::debug!(space = %message.channel_id, "Google Chat message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, _channel_id: &str) -> Result<()> {
        // Google Chat doesn't support typing indicators
        Ok(())
    }

    async fn add_reaction(&self, _channel_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
        // Google Chat doesn't support reactions via API
        tracing::debug!("Google Chat reactions not supported via API");
        Ok(())
    }

    async fn remove_reaction(
        &self,
        _channel_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<()> {
        // Google Chat doesn't support reactions via API
        Ok(())
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

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
