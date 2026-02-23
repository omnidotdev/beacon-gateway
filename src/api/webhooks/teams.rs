//! Microsoft Teams webhook handler
//!
//! Receives Bot Framework activities from Microsoft Teams

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api::ApiState;
use crate::channels::TeamsActivity;
use crate::context::ContextBuilder;
use crate::db::MessageRole;

/// Teams webhook response
#[derive(Serialize)]
pub struct WebhookResponse {
    pub ok: bool,
}

/// Handle incoming Teams Bot Framework activity
#[allow(clippy::too_many_lines)]
pub async fn handle_activity(
    State(state): State<Arc<ApiState>>,
    Json(activity): Json<TeamsActivity>,
) -> (StatusCode, Json<WebhookResponse>) {
    tracing::debug!(
        activity_type = %activity.activity_type,
        id = ?activity.id,
        "received Teams activity"
    );

    // Only handle message activities
    if activity.activity_type != "message" {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    let Some(text) = &activity.text else {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    let Some(conversation) = &activity.conversation else {
        tracing::warn!("Teams activity without conversation");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    let Some(service_url) = &activity.service_url else {
        tracing::warn!("Teams activity without serviceUrl");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    let Some(from) = &activity.from else {
        tracing::warn!("Teams activity without from");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Check if we have Synapse configured
    let Some(synapse) = &state.synapse else {
        tracing::warn!("no Synapse client configured for Teams webhook");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Check if Teams channel is configured
    let Some(teams) = &state.teams else {
        tracing::warn!("no Teams client configured");
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    };

    // Skip bot's own messages
    if from.id == teams.bot_id() {
        return (StatusCode::OK, Json(WebhookResponse { ok: true }));
    }

    let sender_id = from.id.clone();
    let sender_name = from.name.clone().unwrap_or_else(|| sender_id.clone());

    tracing::info!(
        conversation_id = %conversation.id,
        from = %sender_name,
        text = %text,
        "Teams message received"
    );

    // Find or create user and session
    let user = match state.user_repo.find_or_create(&sender_id) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "failed to find/create user");
            return (StatusCode::OK, Json(WebhookResponse { ok: true }));
        }
    };

    let channel_id = conversation.id.clone();
    let session = match state.session_repo.find_or_create(
        &user.id,
        "teams",
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

    // Send response via Teams using the service URL from the activity
    let outgoing = crate::channels::OutgoingMessage::reply(
        conversation.id.clone(),
        response,
        activity.id.clone().unwrap_or_default(),
    );
    if let Err(e) = teams
        .send_to_conversation(service_url, &conversation.id, &outgoing)
        .await
    {
        tracing::error!(error = %e, "failed to send Teams response");
    }

    (StatusCode::OK, Json(WebhookResponse { ok: true }))
}
