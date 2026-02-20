//! Vortex webhook handler for scheduled job callbacks

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentRunConfig, run_agent_turn};
use crate::api::ApiState;
use crate::context::ContextBuilder;
use crate::db::MessageRole;

/// Vortex callback payload
#[derive(Debug, Deserialize)]
pub struct VortexCallback {
    /// Unique identifier for the schedule
    pub schedule_id: String,
    /// Action type to execute
    pub action: String,
    /// Action-specific payload data
    pub payload: serde_json::Value,
    /// ISO 8601 timestamp when the job fired
    pub fired_at: String,
}

/// Vortex webhook response
#[derive(Serialize)]
pub struct VortexResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Payload for the `agent_run` action
#[derive(Debug, Deserialize)]
pub struct AgentRunPayload {
    pub user_id: String,
    pub prompt: String,
    pub channel: String,
    pub channel_id: String,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Optional persona override (defaults to `state.persona_id`)
    #[serde(default)]
    pub persona_id: Option<String>,
}

/// Channel to deliver agent results to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryChannel {
    Telegram,
    Discord,
    Slack,
    WsPush,
}

impl std::str::FromStr for DeliveryChannel {
    type Err = crate::Error;

    fn from_str(s: &str) -> crate::Result<Self> {
        match s {
            "telegram" => Ok(Self::Telegram),
            "discord" => Ok(Self::Discord),
            "slack" => Ok(Self::Slack),
            "ws_push" => Ok(Self::WsPush),
            other => Err(crate::Error::Config(format!("unknown delivery channel: {other}"))),
        }
    }
}

/// Handle Vortex webhook callback
pub async fn handle_vortex_callback(
    State(state): State<Arc<ApiState>>,
    Json(callback): Json<VortexCallback>,
) -> (StatusCode, Json<VortexResponse>) {
    tracing::info!(
        schedule_id = %callback.schedule_id,
        action = %callback.action,
        fired_at = %callback.fired_at,
        "Vortex callback received"
    );

    let result = match callback.action.as_str() {
        "remind" => handle_remind(&state, &callback).await,
        "check_in" => handle_check_in(&state, &callback).await,
        "agent_run" => handle_agent_run(&state, &callback).await,
        _ => {
            tracing::warn!(action = %callback.action, "unknown Vortex action");
            Ok(())
        }
    };

    match result {
        Ok(()) => (StatusCode::OK, Json(VortexResponse { ok: true, error: None })),
        Err(e) => {
            tracing::error!(error = %e, "Vortex callback handler failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(VortexResponse {
                    ok: false,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

/// Handle remind action
async fn handle_remind(state: &ApiState, callback: &VortexCallback) -> crate::Result<()> {
    let user_id = callback
        .payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::Error::Config("missing user_id in remind payload".to_string()))?;

    let message = callback
        .payload
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::Error::Config("missing message in remind payload".to_string()))?;

    let channel = callback
        .payload
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("telegram");

    let channel_id = callback
        .payload
        .get("channel_id")
        .and_then(|v| v.as_str());

    tracing::info!(
        user_id = %user_id,
        channel = %channel,
        message = %message,
        "processing remind action"
    );

    // Route to appropriate channel
    match channel {
        "telegram" => {
            let Some(telegram) = &state.telegram else {
                return Err(crate::Error::Channel("Telegram not configured".to_string()));
            };

            let chat_id = channel_id
                .ok_or_else(|| crate::Error::Config("missing channel_id for Telegram".to_string()))?
                .parse::<i64>()
                .map_err(|e| crate::Error::Config(format!("invalid Telegram chat_id: {e}")))?;

            telegram.send_message(chat_id, message, None).await?;
        }
        _ => {
            tracing::warn!(channel = %channel, "unsupported channel for remind");
        }
    }

    Ok(())
}

/// Handle `check_in` action
async fn handle_check_in(state: &ApiState, callback: &VortexCallback) -> crate::Result<()> {
    let user_id = callback
        .payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::Error::Config("missing user_id in check_in payload".to_string()))?;

    let channel = callback
        .payload
        .get("channel")
        .and_then(|v| v.as_str())
        .unwrap_or("telegram");

    let channel_id = callback
        .payload
        .get("channel_id")
        .and_then(|v| v.as_str());

    let prompt = callback
        .payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("Hey, just checking in. How are you doing?");

    tracing::info!(
        user_id = %user_id,
        channel = %channel,
        "processing check_in action"
    );

    // Get user for context building
    let user = state.user_repo.find_or_create(user_id)?;

    // Generate check-in message via Synapse if available
    let check_in_message = if let Some(synapse) = &state.synapse {
        // Find or create session for this channel
        let channel_id_str = channel_id.unwrap_or(user_id);
        let session = state.session_repo.find_or_create(
            &user.id,
            channel,
            channel_id_str,
            &state.persona_id,
        )?;

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
            Some((&state.memory_repo, prompt)),
        );

        let augmented_prompt = built_context
            .as_ref()
            .map_or_else(|_| prompt.to_string(), |ctx| ctx.format_prompt(prompt));

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

        match synapse.chat_completion(&request).await {
            Ok(resp) => {
                let response = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.content.clone())
                    .unwrap_or_default();
                // Store assistant response
                if let Err(e) = state
                    .session_repo
                    .add_message(&session.id, MessageRole::Assistant, &response)
                {
                    tracing::warn!(error = %e, "failed to store check-in message");
                }
                response
            }
            Err(e) => {
                tracing::error!(error = %e, "synapse error during check-in");
                prompt.to_string()
            }
        }
    } else {
        prompt.to_string()
    };

    // Send via appropriate channel
    match channel {
        "telegram" => {
            let Some(telegram) = &state.telegram else {
                return Err(crate::Error::Channel("Telegram not configured".to_string()));
            };

            let chat_id = channel_id
                .ok_or_else(|| crate::Error::Config("missing channel_id for Telegram".to_string()))?
                .parse::<i64>()
                .map_err(|e| crate::Error::Config(format!("invalid Telegram chat_id: {e}")))?;

            telegram.send_message(chat_id, &check_in_message, None).await?;
        }
        _ => {
            tracing::warn!(channel = %channel, "unsupported channel for check_in");
        }
    }

    Ok(())
}

/// Handle `agent_run` action — runs a full agentic turn and delivers the result to a channel
async fn handle_agent_run(state: &ApiState, callback: &VortexCallback) -> crate::Result<()> {
    let payload: AgentRunPayload = serde_json::from_value(callback.payload.clone())
        .map_err(|e| crate::Error::Config(format!("invalid agent_run payload: {e}")))?;

    tracing::info!(
        user_id = %payload.user_id,
        channel = %payload.channel,
        "processing agent_run action"
    );

    // Find or create user
    let user = state.user_repo.find_or_create(&payload.user_id)?;

    // Resolve persona
    let persona_id = payload.persona_id.as_deref().unwrap_or(&state.persona_id);
    let system_prompt = state.persona_system_prompt.clone().unwrap_or_default();

    // Find or create session for this channel
    let session = state.session_repo.find_or_create(
        &user.id,
        &payload.channel,
        &payload.channel_id,
        persona_id,
    )?;

    // Build context with memory
    let context_config = crate::api::ApiServer::context_config(
        persona_id,
        state.persona_system_prompt.clone(),
    );
    let context_builder = ContextBuilder::new(context_config);
    let built_context = context_builder.build_with_memory(
        &session.id,
        &user.id,
        user.life_json_path.as_deref(),
        &state.session_repo,
        &state.user_repo,
        Some((&state.memory_repo, &payload.prompt)),
    );

    let augmented_prompt = built_context
        .as_ref()
        .map_or_else(|_| payload.prompt.clone(), |ctx| ctx.format_prompt(&payload.prompt));

    let max_iterations = payload.max_iterations.unwrap_or(10);

    let agent_config = AgentRunConfig {
        prompt: augmented_prompt,
        system_prompt,
        model: state.llm_model.clone(),
        max_tokens: state.llm_max_tokens,
        max_iterations,
        session_id: session.id.clone(),
        user_id: user.id.clone(),
        notify: None,
        synapse_override: None,
    };

    let result = run_agent_turn(state, agent_config).await?;

    // Deliver result to the specified channel
    let delivery_channel: DeliveryChannel = payload.channel.parse()?;
    deliver_agent_result(state, delivery_channel, &payload.channel_id, &result).await
}

/// Deliver the agent result to a channel
async fn deliver_agent_result(
    state: &ApiState,
    channel: DeliveryChannel,
    channel_id: &str,
    message: &str,
) -> crate::Result<()> {
    match channel {
        DeliveryChannel::Telegram => {
            let Some(telegram) = &state.telegram else {
                return Err(crate::Error::Channel("Telegram not configured".to_string()));
            };
            let chat_id = channel_id
                .parse::<i64>()
                .map_err(|e| crate::Error::Config(format!("invalid Telegram chat_id: {e}")))?;
            telegram.send_message(chat_id, message, None).await?;
        }
        DeliveryChannel::Discord => {
            tracing::warn!(channel_id, "Discord delivery not yet wired into ApiState");
        }
        DeliveryChannel::Slack => {
            tracing::warn!(channel_id, "Slack delivery not yet wired into ApiState");
        }
        DeliveryChannel::WsPush => {
            if let Some(senders) = &state.ws_senders {
                let map = senders.read().await;
                if let Some(tx) = map.get(channel_id) {
                    let msg = crate::api::websocket::WsOutgoing::ChatChunk {
                        content: message.to_string(),
                    };
                    if tx.send(msg).await.is_err() {
                        tracing::warn!(user_id = channel_id, "ws_push: client disconnected");
                    }
                } else {
                    tracing::warn!(user_id = channel_id, "ws_push: no active connection");
                }
            } else {
                tracing::warn!("ws_push: ws_senders not configured in ApiState");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_run_payload_deserializes() {
        let json = r#"{
            "user_id": "u1",
            "prompt": "What is 2+2?",
            "channel": "telegram",
            "channel_id": "123456",
            "max_iterations": 5
        }"#;
        let payload: AgentRunPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.user_id, "u1");
        assert_eq!(payload.max_iterations, Some(5));
    }

    #[test]
    fn agent_run_payload_defaults_max_iterations() {
        let json = r#"{"user_id":"u1","prompt":"hi","channel":"telegram","channel_id":"1"}"#;
        let payload: AgentRunPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.max_iterations, None);
    }

    #[test]
    fn delivery_channel_parsed_from_string() {
        assert_eq!("telegram".parse::<DeliveryChannel>().unwrap(), DeliveryChannel::Telegram);
        assert_eq!("discord".parse::<DeliveryChannel>().unwrap(), DeliveryChannel::Discord);
        assert_eq!("slack".parse::<DeliveryChannel>().unwrap(), DeliveryChannel::Slack);
        assert_eq!("ws_push".parse::<DeliveryChannel>().unwrap(), DeliveryChannel::WsPush);
        assert!("unknown".parse::<DeliveryChannel>().is_err());
    }
}
