//! Telegram media extraction from webhook messages

use super::types::TelegramMessage;

/// Media file reference for download
pub(crate) struct WebhookMediaRef {
    pub file_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
}

/// Extract media file references from a webhook message
pub(crate) fn extract_media_file_refs(message: &TelegramMessage) -> Vec<WebhookMediaRef> {
    let mut refs = Vec::new();

    // Photo: pick largest size (last in array)
    if let Some(photos) = &message.photo {
        if let Some(largest) = photos.last() {
            refs.push(WebhookMediaRef {
                file_id: largest.file_id.clone(),
                mime_type: "image/jpeg".to_string(),
                filename: None,
            });
        }
    }

    if let Some(doc) = &message.document {
        refs.push(WebhookMediaRef {
            file_id: doc.file_id.clone(),
            mime_type: doc
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            filename: doc.file_name.clone(),
        });
    }

    if let Some(voice) = &message.voice {
        refs.push(WebhookMediaRef {
            file_id: voice.file_id.clone(),
            mime_type: voice
                .mime_type
                .clone()
                .unwrap_or_else(|| "audio/ogg".to_string()),
            filename: None,
        });
    }

    if let Some(audio) = &message.audio {
        refs.push(WebhookMediaRef {
            file_id: audio.file_id.clone(),
            mime_type: audio
                .mime_type
                .clone()
                .unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: audio.file_name.clone(),
        });
    }

    // Sticker
    if let Some(ref sticker) = message.sticker {
        refs.push(WebhookMediaRef {
            file_id: sticker.file_id.clone(),
            mime_type: if sticker.is_animated {
                "application/x-tgsticker".to_string()
            } else if sticker.is_video {
                "video/webm".to_string()
            } else {
                "image/webp".to_string()
            },
            filename: sticker.set_name.as_ref().map(|s| format!("{s}.webp")),
        });
    }

    if let Some(video) = &message.video {
        refs.push(WebhookMediaRef {
            file_id: video.file_id.clone(),
            mime_type: video
                .mime_type
                .clone()
                .unwrap_or_else(|| "video/mp4".to_string()),
            filename: video.file_name.clone(),
        });
    }

    refs
}
