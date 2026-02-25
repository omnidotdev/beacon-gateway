//! Telegram polling mode — getUpdates loop and message conversion

use serde::Deserialize;
use tokio::sync::mpsc;

use super::dedup::UpdateDedup;
use super::types::{API_BASE, MediaFileRef};
use crate::channels::IncomingMessage;

/// Response from Telegram getUpdates API
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    #[allow(dead_code)]
    ok: bool,
    result: Vec<PollingUpdate>,
}

/// A single update from getUpdates
#[derive(Debug, Deserialize)]
struct PollingUpdate {
    update_id: i64,
    message: Option<PollingMessage>,
}

/// Message from a polling update
#[derive(Debug, Deserialize)]
struct PollingMessage {
    message_id: i64,
    chat: PollingChat,
    from: Option<PollingUser>,
    text: Option<String>,
    caption: Option<String>,
    message_thread_id: Option<i64>,
    photo: Option<Vec<PollingPhotoSize>>,
    document: Option<PollingDocument>,
    voice: Option<PollingVoice>,
    audio: Option<PollingAudio>,
    video: Option<PollingVideo>,
    #[allow(dead_code)]
    reply_to_message: Option<Box<serde_json::Value>>,
}

/// Photo size from polling
#[derive(Debug, Deserialize)]
struct PollingPhotoSize {
    file_id: String,
}

/// Document from polling
#[derive(Debug, Deserialize)]
struct PollingDocument {
    file_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
}

/// Voice message from polling
#[derive(Debug, Deserialize)]
struct PollingVoice {
    file_id: String,
    mime_type: Option<String>,
}

/// Audio from polling
#[derive(Debug, Deserialize)]
struct PollingAudio {
    file_id: String,
    title: Option<String>,
    mime_type: Option<String>,
}

/// Video from polling
#[derive(Debug, Deserialize)]
struct PollingVideo {
    file_id: String,
    mime_type: Option<String>,
}

/// Chat info from polling
#[derive(Debug, Deserialize)]
struct PollingChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

/// User info from polling
#[derive(Debug, Deserialize)]
struct PollingUser {
    id: i64,
    is_bot: bool,
    first_name: String,
}

impl super::TelegramChannel {
    /// Spawn a background task that polls Telegram's getUpdates API
    ///
    /// Polls every `interval` and forwards received messages into the mpsc channel.
    /// Deletes any existing webhook before starting to avoid conflicts.
    pub fn start_polling(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let token = self.token.clone();
        let client = self.client.clone();
        let tx = self
            .message_tx
            .clone()
            .expect("start_polling requires a message_tx (use with_receiver)");

        tokio::spawn(async move {
            polling_loop(token, client, tx, interval).await;
        })
    }
}

/// Run the polling loop (background task)
async fn polling_loop(
    token: String,
    client: reqwest::Client,
    tx: mpsc::Sender<IncomingMessage>,
    interval: std::time::Duration,
) {
    // Delete any existing webhook so getUpdates works
    let delete_url = format!("{API_BASE}{token}/deleteWebhook");
    if let Err(e) = client.post(&delete_url).send().await {
        tracing::warn!(error = %e, "failed to delete Telegram webhook before polling");
    }

    let mut offset: Option<i64> = None;
    let mut dedup = UpdateDedup::default();

    loop {
        let url = format!("{API_BASE}{token}/getUpdates");
        let mut params = serde_json::json!({
            "timeout": 30,
            "allowed_updates": ["message"],
        });
        if let Some(off) = offset {
            params["offset"] = serde_json::json!(off);
        }

        match client.post(&url).json(&params).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.text().await {
                    if let Ok(updates) = serde_json::from_str::<GetUpdatesResponse>(&body) {
                        for update in &updates.result {
                            // Advance offset past this update
                            offset = Some(update.update_id + 1);

                            // Dedup check
                            let key = format!("poll:{}", update.update_id);
                            if dedup.is_duplicate(&key) {
                                continue;
                            }

                            if let Some(msg) = update_to_incoming(update) {
                                if let Err(e) = tx.send(msg).await {
                                    tracing::warn!(error = %e, "failed to forward Telegram message");
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Telegram getUpdates error");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Extract media file references from a polling message
fn extract_media_refs(msg: &PollingMessage) -> Vec<MediaFileRef> {
    let mut refs = Vec::new();

    // Photo: pick largest size (last in array)
    if let Some(photos) = &msg.photo {
        if let Some(largest) = photos.last() {
            refs.push(MediaFileRef {
                file_id: largest.file_id.clone(),
                mime_type: "image/jpeg".to_string(),
                filename: None,
            });
        }
    }

    if let Some(doc) = &msg.document {
        refs.push(MediaFileRef {
            file_id: doc.file_id.clone(),
            mime_type: doc.mime_type.clone().unwrap_or_else(|| "application/octet-stream".to_string()),
            filename: doc.file_name.clone(),
        });
    }

    if let Some(voice) = &msg.voice {
        refs.push(MediaFileRef {
            file_id: voice.file_id.clone(),
            mime_type: voice.mime_type.clone().unwrap_or_else(|| "audio/ogg".to_string()),
            filename: None,
        });
    }

    if let Some(audio) = &msg.audio {
        refs.push(MediaFileRef {
            file_id: audio.file_id.clone(),
            mime_type: audio.mime_type.clone().unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: audio.title.clone(),
        });
    }

    if let Some(video) = &msg.video {
        refs.push(MediaFileRef {
            file_id: video.file_id.clone(),
            mime_type: video.mime_type.clone().unwrap_or_else(|| "video/mp4".to_string()),
            filename: None,
        });
    }

    refs
}

/// Convert a polling update into an `IncomingMessage`
fn update_to_incoming(update: &PollingUpdate) -> Option<IncomingMessage> {
    let msg = update.message.as_ref()?;

    let has_media = msg.photo.is_some()
        || msg.document.is_some()
        || msg.voice.is_some()
        || msg.audio.is_some()
        || msg.video.is_some();

    let text = msg.text.clone().or_else(|| msg.caption.clone());

    // Skip messages with no text and no media
    if text.is_none() && !has_media {
        return None;
    }

    // Skip bot messages
    if msg.from.as_ref().is_some_and(|u| u.is_bot) {
        return None;
    }

    // Build content with media annotations as fallback text
    let content = if has_media {
        let mut parts = Vec::new();
        if let Some(ref t) = text {
            parts.push(t.clone());
        }
        if msg.photo.is_some() {
            parts.push("[Photo attached]".to_string());
        }
        if let Some(doc) = &msg.document {
            parts.push(format!(
                "[Document: {}]",
                doc.file_name.as_deref().unwrap_or("file")
            ));
        }
        if msg.audio.is_some() {
            parts.push("[Audio attached]".to_string());
        }
        if msg.video.is_some() {
            parts.push("[Video attached]".to_string());
        }
        if msg.voice.is_some() {
            parts.push("[Voice message]".to_string());
        }
        parts.join("\n")
    } else {
        text.unwrap_or_default()
    };

    let sender_id = msg
        .from
        .as_ref()
        .map_or_else(|| msg.chat.id.to_string(), |u| u.id.to_string());

    let sender_name = msg
        .from
        .as_ref()
        .map_or_else(|| "Unknown".to_string(), |u| u.first_name.clone());

    let is_dm = msg.chat.chat_type == "private";

    Some(IncomingMessage {
        id: msg.message_id.to_string(),
        channel_id: msg.chat.id.to_string(),
        sender_id,
        sender_name,
        content,
        is_dm,
        reply_to: None,
        attachments: vec![],
        thread_id: msg.message_thread_id.map(|id| id.to_string()),
        callback_data: None,
    })
}

/// Extract media refs from a polling update (for download by caller)
#[allow(private_interfaces)]
pub fn extract_update_media_refs(update: &PollingUpdate) -> Vec<MediaFileRef> {
    update
        .message
        .as_ref()
        .map(extract_media_refs)
        .unwrap_or_default()
}
