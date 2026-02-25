//! Telegram Bot API request/response types

use serde::{Deserialize, Serialize};

/// Telegram Bot API base URL
pub(crate) const API_BASE: &str = "https://api.telegram.org/bot";

/// Telegram file download base URL
pub(crate) const FILE_BASE: &str = "https://api.telegram.org/file/bot";

/// Inline keyboard markup for message buttons
#[derive(Debug, Clone, Serialize)]
pub(crate) struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

/// A button in an inline keyboard row
#[derive(Debug, Clone, Serialize)]
pub(crate) struct InlineKeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Telegram sendMessage request
#[derive(Serialize)]
pub(crate) struct SendMessageRequest {
    pub chat_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_web_page_preview: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

/// Telegram setWebhook request
#[derive(Serialize)]
pub(crate) struct SetWebhookRequest {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_updates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_token: Option<String>,
}

/// Telegram sendChatAction request
#[derive(Serialize)]
pub(crate) struct SendChatActionRequest {
    pub chat_id: i64,
    pub action: String,
}

/// Telegram editMessageText request
#[derive(Serialize)]
pub(crate) struct EditMessageTextRequest {
    pub chat_id: i64,
    pub message_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

/// Telegram answerCallbackQuery request
#[derive(Serialize)]
pub(crate) struct AnswerCallbackQueryRequest {
    pub callback_query_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_alert: Option<bool>,
}

/// Telegram deleteMessage request
#[derive(Serialize)]
pub(crate) struct DeleteMessageRequest {
    pub chat_id: i64,
    pub message_id: i64,
}

/// Telegram setMessageReaction request
#[derive(Serialize)]
pub(crate) struct SetMessageReactionRequest {
    pub chat_id: i64,
    pub message_id: i64,
    pub reaction: Vec<ReactionEmoji>,
    pub is_big: bool,
}

/// A single emoji reaction
#[derive(Serialize)]
pub(crate) struct ReactionEmoji {
    #[serde(rename = "type")]
    pub reaction_type: String,
    pub emoji: String,
}

/// Telegram getFile request
#[derive(Serialize)]
pub(crate) struct GetFileRequest {
    pub file_id: String,
}

/// File metadata from Telegram getFile response
#[derive(Debug, Deserialize)]
pub(crate) struct TelegramFile {
    #[allow(dead_code)]
    pub file_id: String,
    pub file_path: Option<String>,
}

/// Telegram setMyCommands request
#[derive(Serialize)]
pub(crate) struct SetMyCommandsRequest {
    pub commands: Vec<BotCommand>,
}

/// A bot command for Telegram's command menu
#[derive(Debug, Clone, Serialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

/// Response from sendMessage containing the sent message
#[derive(Deserialize)]
pub(crate) struct SentMessage {
    pub message_id: i64,
}

/// Telegram API response wrapper
#[derive(Deserialize)]
#[allow(dead_code)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

/// Media file reference extracted from a Telegram message
#[derive(Debug)]
pub struct MediaFileRef {
    /// Telegram file_id for download
    pub file_id: String,
    /// MIME type (best guess)
    pub mime_type: String,
    /// Original filename if available
    pub filename: Option<String>,
}

/// Telegram sendSticker request
#[derive(Serialize)]
pub(crate) struct SendStickerRequest {
    pub chat_id: i64,
    pub sticker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
}

/// Telegram sendVoice request
#[derive(Serialize)]
pub(crate) struct SendVoiceRequest {
    pub chat_id: i64,
    pub voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
}

/// Telegram sendVideoNote request (circular video messages)
#[derive(Serialize)]
pub(crate) struct SendVideoNoteRequest {
    pub chat_id: i64,
    pub video_note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
}

/// Telegram sendPoll request
#[derive(Serialize)]
pub(crate) struct SendPollRequest {
    pub chat_id: i64,
    pub question: String,
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_anonymous: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
}

/// Sticker metadata from a received message
#[derive(Debug, Deserialize)]
pub struct StickerInfo {
    pub file_id: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub set_name: Option<String>,
    #[serde(default)]
    pub is_animated: bool,
    #[serde(default)]
    pub is_video: bool,
}

/// Forward origin metadata from a forwarded message
#[derive(Debug, Deserialize)]
pub struct ForwardOrigin {
    #[serde(rename = "type")]
    pub origin_type: String,
    #[serde(default)]
    pub sender_user_name: Option<String>,
    #[serde(default)]
    pub date: Option<i64>,
}
