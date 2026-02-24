//! Microsoft Teams channel adapter using Graph API and Bot Framework
//!
//! Uses OAuth 2.0 client credentials flow for authentication

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use super::{Attachment, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// Microsoft Teams channel adapter
#[derive(Clone)]
pub struct TeamsChannel {
    tenant_id: String,
    client_id: String,
    client_secret: String,
    bot_id: String,
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

/// OAuth token response
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

/// Bot Framework Activity
#[derive(Debug, Deserialize)]
pub struct TeamsActivity {
    /// Activity type (message, conversationUpdate, etc.)
    #[serde(rename = "type")]
    pub activity_type: String,
    /// Activity ID
    pub id: Option<String>,
    /// Timestamp
    pub timestamp: Option<String>,
    /// Service URL for replies
    #[serde(rename = "serviceUrl")]
    pub service_url: Option<String>,
    /// Channel ID
    #[serde(rename = "channelId")]
    pub channel_id: Option<String>,
    /// Conversation info
    pub conversation: Option<ConversationAccount>,
    /// Sender
    pub from: Option<ChannelAccount>,
    /// Recipient
    pub recipient: Option<ChannelAccount>,
    /// Message text
    pub text: Option<String>,
    /// Reply to ID
    #[serde(rename = "replyToId")]
    pub reply_to_id: Option<String>,
    /// Attachments
    pub attachments: Option<Vec<TeamsAttachment>>,
}

/// Teams attachment from Bot Framework
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsAttachment {
    /// MIME type
    pub content_type: Option<String>,
    /// Download URL
    pub content_url: Option<String>,
    /// Filename
    pub name: Option<String>,
}

/// Conversation account
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConversationAccount {
    /// Conversation ID
    pub id: String,
    /// Conversation type
    #[serde(rename = "conversationType")]
    pub conversation_type: Option<String>,
    /// Tenant ID
    #[serde(rename = "tenantId")]
    pub tenant_id: Option<String>,
    /// Conversation name
    pub name: Option<String>,
}

/// Channel account (user or bot)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChannelAccount {
    /// Account ID
    pub id: String,
    /// Display name
    pub name: Option<String>,
    /// AAD object ID
    #[serde(rename = "aadObjectId")]
    pub aad_object_id: Option<String>,
}

/// Outgoing activity for sending messages
#[derive(Debug, Serialize)]
struct OutgoingActivity<'a> {
    #[serde(rename = "type")]
    activity_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "replyToId")]
    reply_to_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<AdaptiveCardAttachment>>,
}

/// Adaptive Card attachment wrapper
#[derive(Debug, Serialize)]
struct AdaptiveCardAttachment {
    #[serde(rename = "contentType")]
    content_type: &'static str,
    content: AdaptiveCard,
}

/// Adaptive Card structure
#[derive(Debug, Serialize)]
struct AdaptiveCard {
    #[serde(rename = "type")]
    card_type: &'static str,
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    body: Vec<AdaptiveCardElement>,
}

/// Adaptive Card elements
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AdaptiveCardElement {
    TextBlock {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrap: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "fontType")]
        font_type: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        weight: Option<&'static str>,
    },
    Container {
        items: Vec<AdaptiveCardElement>,
        #[serde(skip_serializing_if = "Option::is_none")]
        style: Option<&'static str>,
    },
}

/// Reply context for sending responses
#[derive(Debug, Clone)]
pub struct ReplyContext {
    pub service_url: String,
    pub conversation: ConversationAccount,
}

impl TeamsChannel {
    /// Create a new Microsoft Teams channel adapter
    ///
    /// # Arguments
    ///
    /// * `tenant_id` - Azure AD tenant ID
    /// * `client_id` - Bot application (client) ID
    /// * `client_secret` - Bot client secret
    /// * `bot_id` - Bot ID (usually same as `client_id`)
    #[must_use]
    pub fn new(
        tenant_id: String,
        client_id: String,
        client_secret: String,
        bot_id: String,
    ) -> Self {
        Self {
            tenant_id,
            client_id,
            client_secret,
            bot_id,
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
        tenant_id: String,
        client_id: String,
        client_secret: String,
        bot_id: String,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            tenant_id,
            client_id,
            client_secret,
            bot_id,
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

    /// Get bot ID
    #[must_use]
    pub fn bot_id(&self) -> &str {
        &self.bot_id
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
        let token_url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );

        let response = self
            .client
            .post(&token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("scope", "https://api.botframework.com/.default"),
            ])
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Teams token request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Teams token request failed: {status} - {body}"
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| Error::Channel(format!("Teams token parse error: {e}")))?;

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

    /// Process an incoming Bot Framework activity (from webhook)
    ///
    /// # Errors
    ///
    /// Returns error if message forwarding fails
    pub async fn handle_activity(&self, activity: &TeamsActivity) -> Result<Option<ReplyContext>> {
        // Only handle message activities
        if activity.activity_type != "message" {
            tracing::debug!(activity_type = %activity.activity_type, "Ignoring non-message activity");
            return Ok(None);
        }

        // Skip bot messages
        if let Some(from) = &activity.from {
            if from.id == self.bot_id {
                return Ok(None);
            }
        }

        let conversation = activity
            .conversation
            .as_ref()
            .ok_or_else(|| Error::Channel("Activity without conversation".to_string()))?;

        let from = activity.from.as_ref();
        let sender_id = from.map_or_else(String::new, |f| f.id.clone());
        let sender_name = from
            .and_then(|f| f.name.clone())
            .unwrap_or_else(|| sender_id.clone());

        let is_dm = conversation
            .conversation_type
            .as_deref()
            .is_some_and(|t| t == "personal");

        // Parse attachments
        let attachments = activity
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .filter_map(|att| {
                        let content_url = att.content_url.clone()?;
                        let mime_type = att
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string());
                        Some(Attachment::from_url(content_url, mime_type, att.name.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let incoming = IncomingMessage {
            id: activity.id.clone().unwrap_or_default(),
            channel_id: conversation.id.clone(),
            sender_id,
            sender_name,
            content: activity.text.clone().unwrap_or_default(),
            is_dm,
            reply_to: activity.reply_to_id.clone(),
            attachments,
            thread_id: None,
            callback_data: None,
        };

        if let Some(tx) = &self.message_tx {
            tx.send(incoming)
                .await
                .map_err(|e| Error::Channel(format!("Failed to forward message: {e}")))?;
        }

        // Return reply context for webhook handler
        let reply_context = activity.service_url.as_ref().map(|service_url| ReplyContext {
            service_url: service_url.clone(),
            conversation: conversation.clone(),
        });

        Ok(reply_context)
    }

    /// Send a message using Bot Framework API
    ///
    /// # Errors
    ///
    /// Returns error if token retrieval or message send fails
    pub async fn send_to_conversation(
        &self,
        service_url: &str,
        conversation_id: &str,
        message: &OutgoingMessage,
    ) -> Result<()> {
        let access_token = self.get_access_token().await?;

        let url = format!(
            "{}/v3/conversations/{}/activities",
            service_url.trim_end_matches('/'),
            conversation_id
        );

        let activity = build_teams_activity(message);

        let response = self
            .client
            .post(&url)
            .bearer_auth(&access_token)
            .json(&activity)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Teams send failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Teams send failed: {status} - {body}"
            )));
        }

        tracing::debug!(conversation = %conversation_id, "Teams message sent");
        Ok(())
    }
}

/// Build a Teams activity from an outgoing message
fn build_teams_activity(message: &OutgoingMessage) -> OutgoingActivity<'_> {
    if message.has_code_blocks() {
        // Build Adaptive Card for code blocks
        let mut body = Vec::new();
        let code_blocks = message.extract_code_blocks();
        let plain_content = remove_code_blocks(&message.content);

        // Add plain text if present
        if !plain_content.trim().is_empty() {
            body.push(AdaptiveCardElement::TextBlock {
                text: plain_content.trim().to_string(),
                wrap: Some(true),
                font_type: None,
                size: None,
                weight: None,
            });
        }

        // Add code blocks with monospace font
        for (lang, code) in code_blocks {
            // Truncate if too long (Teams has size limits)
            let truncated = if code.len() > 8000 {
                format!("{}...\n(truncated)", &code[..8000])
            } else {
                code
            };

            // Add language header if specified
            if !lang.is_empty() {
                body.push(AdaptiveCardElement::TextBlock {
                    text: lang,
                    wrap: None,
                    font_type: None,
                    size: Some("Small"),
                    weight: Some("Bolder"),
                });
            }

            // Add code in a container with monospace font
            body.push(AdaptiveCardElement::Container {
                items: vec![AdaptiveCardElement::TextBlock {
                    text: truncated,
                    wrap: Some(true),
                    font_type: Some("Monospace"),
                    size: None,
                    weight: None,
                }],
                style: Some("emphasis"),
            });
        }

        let card = AdaptiveCard {
            card_type: "AdaptiveCard",
            schema: "http://adaptivecards.io/schemas/adaptive-card.json",
            version: "1.4",
            body,
        };

        OutgoingActivity {
            activity_type: "message",
            text: Some(&message.content), // Fallback text
            reply_to_id: message.reply_to.as_deref(),
            attachments: Some(vec![AdaptiveCardAttachment {
                content_type: "application/vnd.microsoft.card.adaptive",
                content: card,
            }]),
        }
    } else {
        OutgoingActivity {
            activity_type: "message",
            text: Some(&message.content),
            reply_to_id: message.reply_to.as_deref(),
            attachments: None,
        }
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

#[async_trait]
impl Channel for TeamsChannel {
    fn name(&self) -> &'static str {
        "teams"
    }

    async fn connect(&mut self) -> Result<()> {
        // Verify credentials by getting initial access token
        let _token = self.get_access_token().await?;

        tracing::info!(
            tenant_id = %self.tenant_id,
            bot_id = %self.bot_id,
            "Teams authenticated"
        );

        self.connected = true;
        tracing::info!("Teams channel connected");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        tracing::info!("Teams channel disconnected");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        // For Teams, we need the service URL to send messages
        // This is provided via webhooks, so direct send requires proactive messaging setup
        // For now, log a warning - replies should use send_to_conversation via webhook handler
        tracing::warn!(
            channel_id = %message.channel_id,
            "Direct Teams send requires proactive messaging setup; use webhook reply context"
        );

        // Attempt proactive message via Bot Framework
        // This requires the conversation to have been previously established
        let access_token = self.get_access_token().await?;

        // Try common Bot Framework service URL
        let service_url = "https://smba.trafficmanager.net/teams";
        let url = format!(
            "{}/v3/conversations/{}/activities",
            service_url,
            message.channel_id
        );

        let activity = build_teams_activity(&message);

        let response = self
            .client
            .post(&url)
            .bearer_auth(&access_token)
            .json(&activity)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Teams send failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Teams send failed: {status} - {body}"
            )));
        }

        tracing::debug!(channel_id = %message.channel_id, "Teams message sent");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn send_typing(&self, _channel_id: &str) -> Result<()> {
        // Teams typing indicator requires sending a "typing" activity type
        // This would need the service URL context from webhook
        tracing::debug!("Teams typing indicator requires webhook context");
        Ok(())
    }

    async fn add_reaction(&self, _channel_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
        // Teams reactions via Graph API require specific permissions
        tracing::debug!("Teams reactions not implemented");
        Ok(())
    }

    async fn remove_reaction(
        &self,
        _channel_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<()> {
        Ok(())
    }
}
