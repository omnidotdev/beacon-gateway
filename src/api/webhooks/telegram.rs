//! Telegram webhook handler

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::api::ApiState;
use crate::context::ContextBuilder;
use crate::db::MessageRole;

/// Telegram Update object (simplified)
#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

/// Telegram Message object (simplified)
#[derive(Debug, Deserialize)]
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
}

/// Telegram photo size
#[derive(Debug, Deserialize)]
pub struct TelegramPhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub file_size: Option<i64>,
}

/// Telegram document
#[derive(Debug, Deserialize)]
pub struct TelegramDocument {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram audio
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct TelegramVoice {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i32,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram Chat object
#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
}

/// Telegram User object
#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

/// Telegram webhook response
#[derive(Serialize)]
pub struct WebhookResponse {
    pub ok: bool,
}

/// Handle incoming Telegram update
#[allow(clippy::too_many_lines)]
pub async fn handle_update(
    State(state): State<Arc<ApiState>>,
    Json(update): Json<TelegramUpdate>,
) -> (StatusCode, Json<WebhookResponse>) {
    tracing::debug!(update_id = update.update_id, "received Telegram update");

    let Some(message) = &update.message else {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Get text content - use caption if no text (for media messages)
    let text = message
        .text
        .clone()
        .or_else(|| message.caption.clone())
        .unwrap_or_default();

    // Check if this is a media message with attachments
    let has_photo = message.photo.is_some();
    let has_document = message.document.is_some();
    let has_audio = message.audio.is_some();
    let has_video = message.video.is_some();
    let has_voice = message.voice.is_some();
    let has_media = has_photo || has_document || has_audio || has_video || has_voice;

    // Skip if no text and no media
    if text.is_empty() && !has_media {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    // Build content with attachment metadata
    let content = if has_media {
        let mut parts = vec![text.clone()];
        if has_photo {
            parts.push("[Photo attached]".to_string());
        }
        if let Some(doc) = &message.document {
            parts.push(format!(
                "[Document: {}]",
                doc.file_name.as_deref().unwrap_or("file")
            ));
        }
        if let Some(audio) = &message.audio {
            parts.push(format!(
                "[Audio: {}]",
                audio.title.as_deref().unwrap_or("audio")
            ));
        }
        if has_video {
            parts.push("[Video attached]".to_string());
        }
        if has_voice {
            parts.push("[Voice message]".to_string());
        }
        parts.join("\n")
    } else {
        text.clone()
    };

    // Ignore bot messages
    if message.from.as_ref().is_some_and(|u| u.is_bot) {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    let sender_id = message
        .from
        .as_ref()
        .map_or_else(|| message.chat.id.to_string(), |u| u.id.to_string());

    let sender_name = message
        .from
        .as_ref()
        .map_or_else(|| "Unknown".to_string(), |u| u.first_name.clone());

    tracing::info!(
        chat_id = message.chat.id,
        from = %sender_name,
        content = %content,
        has_media,
        "Telegram message received"
    );

    // Check if we have Synapse configured
    let Some(synapse) = &state.synapse else {
        tracing::warn!("no Synapse client configured for Telegram webhook");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    let Some(telegram) = &state.telegram else {
        tracing::warn!("no Telegram client configured");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Find or create user and session
    let user = match state.user_repo.find_or_create(&sender_id) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "failed to find/create user");
            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }
    };

    let channel_id = message.chat.id.to_string();
    let session = match state.session_repo.find_or_create(
        &user.id,
        "telegram",
        &channel_id,
        &state.persona_id,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to find/create session");
            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }
    };

    // Store user message
    if let Err(e) = state
        .session_repo
        .add_message(&session.id, MessageRole::User, &content)
    {
        tracing::warn!(error = %e, "failed to store user message");
    }

    // Build context with memory
    let context_config = crate::api::ApiServer::context_config(
        &state.persona_id,
        state.persona_system_prompt.clone(),
    );
    let context_builder = ContextBuilder::new(context_config);
    let built_context = context_builder.build_with_memory(
        &session.id,
        &user.id,
        user.life_json_path.as_deref(),
        &state.session_repo,
        &state.user_repo,
        Some((&state.memory_repo, &content)),
    );

    if let Ok(ctx) = &built_context {
        tracing::debug!(
            session = %session.id,
            estimated_tokens = ctx.estimated_tokens,
            message_count = ctx.messages.len(),
            "built conversation context"
        );
    }

    // Build augmented prompt with context and history
    let augmented_prompt = built_context
        .as_ref()
        .map_or_else(|_| content.clone(), |ctx| ctx.format_prompt(&content));

    // Process with Synapse
    let request = synapse_client::ChatRequest {
        model: state.llm_model.clone(),
        messages: vec![
            synapse_client::Message::system(&state.system_prompt),
            synapse_client::Message::user(&augmented_prompt),
        ],
        stream: false,
        temperature: None,
        top_p: None,
        max_tokens: Some(state.llm_max_tokens),
        stop: None,
        tools: None,
        tool_choice: None,
    };

    let response = match synapse.chat_completion(&request).await {
        Ok(resp) => resp
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default(),
        Err(e) => {
            tracing::error!(error = %e, "synapse error");
            "Sorry, I encountered an error processing your message.".to_string()
        }
    };

    // Store assistant response
    if let Err(e) = state
        .session_repo
        .add_message(&session.id, MessageRole::Assistant, &response)
    {
        tracing::warn!(error = %e, "failed to store assistant message");
    }

    // Send response via Telegram (reply to original message)
    if let Err(e) = telegram.send_message(message.chat.id, &response, Some(message.message_id)).await {
        tracing::error!(error = %e, "failed to send Telegram response");
    }

    (StatusCode::OK, Json(WebhookResponse { ok: true }))
}
