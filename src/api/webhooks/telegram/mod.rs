//! Telegram webhook handler

mod media;
mod process;
pub mod types;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;

use self::types::TelegramUpdate;
use crate::api::ApiState;

/// Telegram webhook response
#[derive(Serialize)]
pub struct WebhookResponse {
    pub ok: bool,
}

/// Handle incoming Telegram update (default account)
///
/// Returns 200 immediately and processes the message in a background task.
/// Telegram requires fast webhook responses to avoid retries.
pub async fn handle_update(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(update): Json<TelegramUpdate>,
) -> (StatusCode, Json<WebhookResponse>) {
    handle_update_for_account(state, headers, update, None).await
}

/// Handle incoming Telegram update for a specific account
///
/// Per-account webhook endpoint: `POST /api/webhooks/telegram/{account_id}`
pub async fn handle_account_update(
    State(state): State<Arc<ApiState>>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(update): Json<TelegramUpdate>,
) -> (StatusCode, Json<WebhookResponse>) {
    handle_update_for_account(state, headers, update, Some(account_id)).await
}

/// Shared handler logic for both default and per-account webhook endpoints
#[allow(clippy::unused_async, clippy::too_many_lines)]
async fn handle_update_for_account(
    state: Arc<ApiState>,
    headers: HeaderMap,
    update: TelegramUpdate,
    account_id: Option<String>,
) -> (StatusCode, Json<WebhookResponse>) {
    // Validate webhook secret token if configured
    if let Some(expected) = state
        .telegram_config
        .as_ref()
        .and_then(|c| c.webhook_secret.as_deref())
    {
        let provided = headers
            .get("x-telegram-bot-api-secret-token")
            .and_then(|v| v.to_str().ok());

        if provided != Some(expected) {
            tracing::warn!("Telegram webhook secret mismatch");
            return (StatusCode::FORBIDDEN, Json(WebhookResponse { ok: false }));
        }
    }

    // Debug logging for raw updates (when TELEGRAM_DEBUG_UPDATES is enabled)
    if state.telegram_config.as_ref().is_some_and(|c| c.debug_updates) {
        match serde_json::to_string(&update) {
            Ok(json) => tracing::debug!(raw = %json, "Telegram raw update"),
            Err(e) => tracing::warn!(error = %e, "failed to serialize update for debug"),
        }
    }

    let label = account_id.as_deref().unwrap_or("default");
    tracing::debug!(update_id = update.update_id, account = label, "received Telegram update");

    // Dedup check — prevent processing the same update twice
    {
        let key = format!("update:{}:{}", label, update.update_id);
        let mut dedup = state.telegram_dedup.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if dedup.is_duplicate(&key) {
            tracing::debug!(update_id = update.update_id, "duplicate Telegram update, skipping");
            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }
    }

    // For per-account requests, verify the account exists in the registry
    if let Some(ref aid) = account_id {
        let account_exists = state
            .telegram_registry
            .as_ref()
            .is_some_and(|r| r.get(aid).is_some());
        if !account_exists {
            tracing::warn!(account = aid, "unknown Telegram account");
            return (StatusCode::NOT_FOUND, Json(WebhookResponse { ok: false }));
        }
    }

    // Handle callback queries (inline keyboard button presses)
    if let Some(callback) = update.callback_query {
        if let (Some(cb_message), Some(cb_data)) = (callback.message, callback.data) {
            let text = cb_data;
            let has_media = false;
            let callback_id = callback.id;

            // Resolve the Telegram channel to answer the callback query
            let telegram = if let Some(ref aid) = account_id {
                state
                    .telegram_registry
                    .as_ref()
                    .and_then(|r| r.get(aid))
                    .map(|a| &a.channel)
            } else {
                state.telegram.as_ref()
            };

            // Answer the callback query to dismiss the loading spinner
            if let Some(tg) = telegram {
                let _ = tg.answer_callback_query(&callback_id, None).await;
            }

            tokio::spawn(async move {
                if let Err(e) = process::process_telegram_message(state, cb_message, text, has_media, account_id).await {
                    tracing::error!(error = %e, "Telegram callback query processing failed");
                }
            });

            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }

        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    let Some(message) = update.message else {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Ignore bot messages
    if message.from.as_ref().is_some_and(|u| u.is_bot) {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    // Resolve bot username for mention gating — per-account > global
    let bot_username = account_id
        .as_ref()
        .and_then(|aid| {
            state
                .telegram_registry
                .as_ref()
                .and_then(|r| r.get(aid))
                .and_then(|a| a.config.bot_username.clone())
        })
        .or_else(|| {
            state
                .telegram_config
                .as_ref()
                .and_then(|c| c.bot_username.clone())
        });

    // Resolve require_mention — per-account > per-group > global
    let global_require_mention = account_id
        .as_ref()
        .and_then(|aid| {
            state
                .telegram_registry
                .as_ref()
                .and_then(|r| r.get(aid))
                .and_then(|a| a.config.require_mention_in_groups)
        })
        .or_else(|| {
            state
                .telegram_config
                .as_ref()
                .map(|c| c.require_mention_in_groups)
        })
        .unwrap_or(false);

    // Mention gating: skip group messages that don't mention the bot
    let is_group_chat = message.chat.chat_type == "group"
        || message.chat.chat_type == "supergroup";

    if is_group_chat {
        let chat_id_str = message.chat.id.to_string();

        // Check per-group config first, fall back to global
        let group_config = state.telegram_group_repo.get(&chat_id_str).ok().flatten();

        // If group is explicitly disabled, skip
        if group_config.as_ref().is_some_and(|gc| !gc.enabled) {
            tracing::debug!(chat_id = message.chat.id, "skipping disabled group");
            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }

        // Determine require_mention: per-group override > per-account/global
        let require_mention = group_config
            .as_ref()
            .and_then(|gc| gc.require_mention)
            .unwrap_or(global_require_mention);

        if require_mention {
            let text_for_check = message
                .text
                .as_deref()
                .or(message.caption.as_deref())
                .unwrap_or("");

            let mentioned = bot_username
                .as_ref()
                .is_some_and(|username| {
                    text_for_check.contains(&format!("@{username}"))
                })
                || message.reply_to_message.is_some();

            if !mentioned {
                tracing::debug!(
                    chat_id = message.chat.id,
                    "skipping group message (bot not mentioned)"
                );
                return (StatusCode::OK, Json(WebhookResponse { ok: true }));
            }
        }
    }

    // Get text content — use caption if no text (for media messages)
    let mut text = message
        .text
        .clone()
        .or_else(|| message.caption.clone())
        .unwrap_or_default();

    let has_media = message.photo.is_some()
        || message.document.is_some()
        || message.audio.is_some()
        || message.video.is_some()
        || message.voice.is_some()
        || message.sticker.is_some();

    // For sticker-only messages, use emoji as content
    if text.is_empty() && message.sticker.is_some() {
        let sticker_emoji = message.sticker.as_ref()
            .and_then(|s| s.emoji.as_deref())
            .unwrap_or("\u{1f3ad}");
        text = format!("[Sticker: {sticker_emoji}]");
    }

    // Add forward context annotation
    if let Some(ref origin) = message.forward_origin {
        let source = origin.sender_user_name.as_deref().unwrap_or("unknown");
        text = format!("[Forwarded from {source}] {text}");
    }

    if text.is_empty() && !has_media {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    // Spawn processing in background so we return 200 immediately
    tokio::spawn(async move {
        if let Err(e) = process::process_telegram_message(state, message, text, has_media, account_id).await {
            tracing::error!(error = %e, "Telegram message processing failed");
        }
    });

    (StatusCode::OK, Json(WebhookResponse { ok: true }))
}
