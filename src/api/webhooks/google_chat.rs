//! Google Chat webhook handler
//!
//! Receives events from Google Chat via Pub/Sub or HTTP push

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api::ApiState;
use crate::channels::GoogleChatEvent;
use crate::context::ContextBuilder;
use crate::db::MessageRole;

/// Google Chat webhook response
#[derive(Serialize)]
pub struct WebhookResponse {
    /// Response text (Google Chat expects this for synchronous replies)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Handle incoming Google Chat event
#[allow(clippy::too_many_lines)]
pub async fn handle_event(
    State(state): State<Arc<ApiState>>,
    Json(event): Json<GoogleChatEvent>,
) -> (StatusCode, Json<WebhookResponse>) {
    tracing::debug!(
        event_type = %event.event_type,
        "received Google Chat event"
    );

    // Handle ADDED_TO_SPACE event
    if event.event_type == "ADDED_TO_SPACE" {
        let space_name = event
            .space
            .as_ref()
            .map_or("unknown", |s| &s.name);
        tracing::info!(space = %space_name, "Bot added to Google Chat space");
        return (
            StatusCode::OK,
            Json(WebhookResponse {
                text: Some("Hello! I'm ready to help.".to_string()),
            }),
        );
    }

    // Only handle MESSAGE events
    if event.event_type != "MESSAGE" {
        return (StatusCode::OK, Json(WebhookResponse { text: None }));
    }

    let Some(message) = &event.message else {
        return (StatusCode::OK, Json(WebhookResponse { text: None }));
    };

    let Some(text) = &message.text else {
        return (StatusCode::OK, Json(WebhookResponse { text: None }));
    };

    let Some(space) = &event.space else {
        tracing::warn!("Google Chat event without space");
        return (StatusCode::OK, Json(WebhookResponse { text: None }));
    };

    // Skip bot messages
    if let Some(sender) = &message.sender {
        if sender.user_type.as_deref() == Some("BOT") {
            return (StatusCode::OK, Json(WebhookResponse { text: None }));
        }
    }

    // Check if we have Synapse configured
    let Some(synapse) = &state.synapse else {
        tracing::warn!("no Synapse client configured for Google Chat webhook");
        return (StatusCode::OK, Json(WebhookResponse { text: None }));
    };

    let sender = message.sender.as_ref();
    let sender_id = sender.map_or_else(String::new, |s| s.name.clone());
    let sender_name = sender
        .and_then(|s| s.display_name.clone())
        .unwrap_or_else(|| sender_id.clone());

    tracing::info!(
        space = %space.name,
        from = %sender_name,
        text = %text,
        "Google Chat message received"
    );

    // Find or create user and session
    let user = match state.user_repo.find_or_create(&sender_id) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "failed to find/create user");
            return (StatusCode::OK, Json(WebhookResponse { text: None }));
        }
    };

    let channel_id = space.name.clone();
    let session = match state.session_repo.find_or_create(
        &user.id,
        "google_chat",
        &channel_id,
        &state.persona_id,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to find/create session");
            return (StatusCode::OK, Json(WebhookResponse { text: None }));
        }
    };

    // Store user message
    if let Err(e) = state
        .session_repo
        .add_message(&session.id, MessageRole::User, text)
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
        Some((&state.memory_repo, text)),
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
        .map_or_else(|_| text.clone(), |ctx| ctx.format_prompt(text));

    // Process with Synapse
    let request = synapse_client::ChatRequest {
        model: state.llm_model.clone(),
        messages: vec![
            synapse_client::Message::system(&state.system_prompt_with_skills(None)),
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

    // Return synchronous response (Google Chat supports this)
    (
        StatusCode::OK,
        Json(WebhookResponse {
            text: Some(response),
        }),
    )
}
