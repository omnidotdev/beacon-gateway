//! Raw Telegram Bot API calls

use super::html::markdown_to_telegram_html;
use super::types::*;
use crate::{Error, Result};

impl super::TelegramChannel {
    /// Send a message to a chat
    ///
    /// Uses HTML parse mode with plain-text fallback.
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_message(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<()> {
        let url = format!("{API_BASE}{}/sendMessage", self.token);

        let html_text = markdown_to_telegram_html(text);
        let request = SendMessageRequest {
            chat_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
            reply_to_message_id: reply_to,
            message_thread_id: None,
            disable_web_page_preview: None,
            disable_notification: None,
            reply_markup: None,
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
            let body_lower = body.to_lowercase();

            // Return a descriptive error for unreachable chats (no point retrying)
            if body_lower.contains("chat not found")
                || body_lower.contains("bot was blocked by the user")
            {
                return Err(Error::Channel(format!(
                    "Telegram chat {chat_id} not reachable: {body}"
                )));
            }

            let fallback_request = SendMessageRequest {
                chat_id,
                text: text.to_string(),
                parse_mode: None,
                reply_to_message_id: reply_to,
                message_thread_id: None,
                disable_web_page_preview: None,
                disable_notification: None,
                reply_markup: None,
            };

            let fallback_response = self
                .client
                .post(&url)
                .json(&fallback_request)
                .send()
                .await
                .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

            if !fallback_response.status().is_success() {
                let fallback_body = fallback_response.text().await.unwrap_or_default();
                let fallback_lower = fallback_body.to_lowercase();

                if fallback_lower.contains("chat not found")
                    || fallback_lower.contains("bot was blocked by the user")
                {
                    return Err(Error::Channel(format!(
                        "Telegram chat {chat_id} not reachable: {fallback_body}"
                    )));
                }

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
    pub async fn set_webhook(&self, url: &str, secret_token: Option<&str>) -> Result<()> {
        let api_url = format!("{API_BASE}{}/setWebhook", self.token);

        let request = SetWebhookRequest {
            url: url.to_string(),
            allowed_updates: Some(vec!["message".to_string(), "callback_query".to_string()]),
            secret_token: secret_token.map(String::from),
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

        let html_text = markdown_to_telegram_html(text);
        let request = SendMessageRequest {
            chat_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
            disable_web_page_preview: None,
            disable_notification: None,
            reply_markup: None,
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

        // If the thread/topic is gone, retry once without thread_id
        if parsed.result.is_none() && thread_id.is_some() {
            let desc = parsed.description.as_deref().unwrap_or_default().to_lowercase();
            if desc.contains("message thread not found")
                || desc.contains("topic_closed")
                || desc.contains("topic_deleted")
            {
                tracing::warn!(
                    chat_id,
                    ?thread_id,
                    "Thread not found, retrying without message_thread_id"
                );

                let retry_request = SendMessageRequest {
                    chat_id,
                    text: markdown_to_telegram_html(text),
                    parse_mode: Some("HTML".to_string()),
                    reply_to_message_id: reply_to,
                    message_thread_id: None,
                    disable_web_page_preview: None,
                    disable_notification: None,
                    reply_markup: None,
                };

                let retry_response = self
                    .client
                    .post(&url)
                    .json(&retry_request)
                    .send()
                    .await
                    .map_err(|e| Error::Channel(format!("Telegram API error: {e}")))?;

                let retry_body = retry_response
                    .text()
                    .await
                    .map_err(|e| Error::Channel(format!("Telegram response read error: {e}")))?;

                let retry_parsed: TelegramResponse<SentMessage> =
                    serde_json::from_str(&retry_body).map_err(|e| {
                        Error::Channel(format!("Telegram response parse error: {e}"))
                    })?;

                return retry_parsed
                    .result
                    .map(|m| m.message_id)
                    .ok_or_else(|| {
                        Error::Channel(format!(
                            "Telegram API error: {}",
                            retry_parsed.description.unwrap_or_default()
                        ))
                    });
            }
        }

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

        let html_text = markdown_to_telegram_html(text);
        let request = EditMessageTextRequest {
            chat_id,
            message_id,
            text: html_text,
            parse_mode: Some("HTML".to_string()),
            reply_markup: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram editMessageText error: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();

            // Suppress "message is not modified" errors (common during streaming)
            if body.to_lowercase().contains("message is not modified") {
                return Ok(());
            }

            // Fallback to plain text on parse error
            let fallback = EditMessageTextRequest {
                chat_id,
                message_id,
                text: text.to_string(),
                parse_mode: None,
                reply_markup: None,
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
                let fallback_body = fallback_resp.text().await.unwrap_or_default();

                // Suppress "message is not modified" in fallback path too
                if fallback_body.to_lowercase().contains("message is not modified") {
                    return Ok(());
                }

                return Err(Error::Channel(format!(
                    "Telegram editMessageText error: {fallback_body}"
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

    /// Send a chat action (typing indicator, etc.)
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<()> {
        let url = format!("{API_BASE}{}/sendChatAction", self.token);

        let request = SendChatActionRequest {
            chat_id,
            action: action.to_string(),
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

        Ok(())
    }

    /// Set a reaction on a message
    ///
    /// Gracefully degrades if the bot lacks admin permission.
    pub async fn set_message_reaction(&self, chat_id: i64, message_id: i64, emoji: &str) -> Result<()> {
        let url = format!("{API_BASE}{}/setMessageReaction", self.token);
        let request = SetMessageReactionRequest {
            chat_id,
            message_id,
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
            tracing::warn!(
                chat_id,
                message_id,
                emoji,
                "Telegram reaction failed (bot may not have permission)"
            );
        }

        Ok(())
    }

    /// Clear all reactions from a message
    pub async fn clear_message_reaction(&self, chat_id: i64, message_id: i64) -> Result<()> {
        let url = format!("{API_BASE}{}/setMessageReaction", self.token);
        let request = SetMessageReactionRequest {
            chat_id,
            message_id,
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
            tracing::warn!(chat_id, message_id, "Telegram remove reaction failed");
        }

        Ok(())
    }

    /// Answer a callback query to dismiss the loading spinner on the button
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn answer_callback_query(&self, callback_query_id: &str, text: Option<&str>) -> Result<()> {
        let url = format!("{API_BASE}{}/answerCallbackQuery", self.token);

        let request = AnswerCallbackQueryRequest {
            callback_query_id: callback_query_id.to_string(),
            text: text.map(String::from),
            show_alert: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram answerCallbackQuery error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram answerCallbackQuery error: {status} - {body}"
            )));
        }

        Ok(())
    }

    /// Validate the bot token by calling `getMe`
    ///
    /// # Errors
    ///
    /// Returns error if the token is invalid
    pub async fn get_me(&self) -> Result<()> {
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

        Ok(())
    }

    /// Send a sticker by file_id
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_sticker(
        &self,
        chat_id: i64,
        sticker_file_id: &str,
        reply_to: Option<i64>,
        thread_id: Option<i64>,
    ) -> Result<()> {
        let url = format!("{API_BASE}{}/sendSticker", self.token);

        let request = SendStickerRequest {
            chat_id,
            sticker: sticker_file_id.to_string(),
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
            disable_notification: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram sendSticker error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram sendSticker error: {status} - {body}"
            )));
        }

        tracing::debug!(chat_id, "Telegram sticker sent");
        Ok(())
    }

    /// Send audio as a voice message (OGG/Opus format)
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_voice(
        &self,
        chat_id: i64,
        voice_file_id: &str,
        caption: Option<&str>,
        reply_to: Option<i64>,
        thread_id: Option<i64>,
    ) -> Result<()> {
        let url = format!("{API_BASE}{}/sendVoice", self.token);

        let request = SendVoiceRequest {
            chat_id,
            voice: voice_file_id.to_string(),
            caption: caption.map(String::from),
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
            disable_notification: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram sendVoice error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram sendVoice error: {status} - {body}"
            )));
        }

        tracing::debug!(chat_id, "Telegram voice message sent");
        Ok(())
    }

    /// Send a circular video note
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_video_note(
        &self,
        chat_id: i64,
        video_note_file_id: &str,
        reply_to: Option<i64>,
        thread_id: Option<i64>,
    ) -> Result<()> {
        let url = format!("{API_BASE}{}/sendVideoNote", self.token);

        let request = SendVideoNoteRequest {
            chat_id,
            video_note: video_note_file_id.to_string(),
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
            disable_notification: None,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram sendVideoNote error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram sendVideoNote error: {status} - {body}"
            )));
        }

        tracing::debug!(chat_id, "Telegram video note sent");
        Ok(())
    }

    /// Send a poll
    ///
    /// # Errors
    ///
    /// Returns error if the API request fails
    pub async fn send_poll(
        &self,
        chat_id: i64,
        question: &str,
        options: &[String],
        reply_to: Option<i64>,
        thread_id: Option<i64>,
    ) -> Result<()> {
        let url = format!("{API_BASE}{}/sendPoll", self.token);

        let request = SendPollRequest {
            chat_id,
            question: question.to_string(),
            options: options.to_vec(),
            is_anonymous: None,
            reply_to_message_id: reply_to,
            message_thread_id: thread_id,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Channel(format!("Telegram sendPoll error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Telegram sendPoll error: {status} - {body}"
            )));
        }

        tracing::debug!(chat_id, "Telegram poll sent");
        Ok(())
    }
}
