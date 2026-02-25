//! Telegram webhook types

use serde::{Deserialize, Serialize};

/// Telegram Update object (simplified)
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub callback_query: Option<TelegramCallbackQuery>,
}

/// Callback query from an inline keyboard button press
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramCallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    pub message: Option<TelegramMessage>,
    pub data: Option<String>,
}

/// Telegram Message object (simplified)
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub date: i64,
    /// Photo (array of sizes, use largest)
    pub photo: Option<Vec<TelegramPhotoSize>>,
    /// Document/file attachment
    pub document: Option<TelegramDocument>,
    /// Audio message
    pub audio: Option<TelegramAudio>,
    /// Video message
    pub video: Option<TelegramVideo>,
    /// Voice message
    pub voice: Option<TelegramVoice>,
    /// Sticker message
    pub sticker: Option<TelegramSticker>,
    /// Forward origin info
    pub forward_origin: Option<TelegramForwardOrigin>,
    /// Forward date
    pub forward_date: Option<i64>,
    /// Forum topic / thread ID
    pub message_thread_id: Option<i64>,
    /// Message this is replying to (for mention gating)
    pub reply_to_message: Option<Box<TelegramReplyMessage>>,
}

/// Minimal reply message (only need to know it exists for mention gating)
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramReplyMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
}

/// Telegram photo size
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramPhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub file_size: Option<i64>,
}

/// Telegram document
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramDocument {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram audio
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramAudio {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i32,
    pub performer: Option<String>,
    pub title: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram video
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramVideo {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram voice message
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramVoice {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i32,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Sticker in a Telegram message
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramSticker {
    pub file_id: String,
    pub file_unique_id: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub set_name: Option<String>,
    #[serde(default)]
    pub is_animated: bool,
    #[serde(default)]
    pub is_video: bool,
}

/// Forward origin metadata
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramForwardOrigin {
    #[serde(rename = "type")]
    pub origin_type: String,
    #[serde(default)]
    pub sender_user_name: Option<String>,
    #[serde(default)]
    pub date: Option<i64>,
}

/// Telegram Chat object
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
}

/// Telegram User object
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}
