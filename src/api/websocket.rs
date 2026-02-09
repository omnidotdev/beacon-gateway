//! WebSocket handler for real-time chat

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use omni_cli::Agent;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use super::ApiState;
use crate::context::ContextBuilder;
use crate::db::MessageRole;

/// Optional query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
struct WsQuery {
    token: Option<String>,
}

/// Incoming WebSocket message from client
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsIncoming {
    /// Send a chat message
    Chat { content: String },
    /// Ping to keep connection alive
    Ping,
}

/// Outgoing WebSocket message to client
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsOutgoing {
    /// Chat response chunk (streamed)
    ChatChunk { content: String },
    /// Chat response complete
    ChatComplete { message_id: String },
    /// Error occurred
    Error { code: String, message: String },
    /// Pong response
    Pong,
    /// Connection established
    Connected { session_id: String },
}

/// Build WebSocket router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/chat/{session_id}", get(ws_upgrade))
        .with_state(state)
}

/// Handle WebSocket upgrade request
async fn ws_upgrade(
    State(state): State<Arc<ApiState>>,
    Path(session_id): Path<String>,
    query: Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let token = query.0.token;
    ws.on_upgrade(move |socket| handle_socket(socket, state, session_id, token))
}

/// Handle WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    state: Arc<ApiState>,
    session_id: String,
    token: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();

    // Send connected message
    let connected = WsOutgoing::Connected {
        session_id: session_id.clone(),
    };
    if let Ok(msg) = serde_json::to_string(&connected) {
        if sender.send(Message::Text(msg.into())).await.is_err() {
            return;
        }
    }

    tracing::info!(session_id = %session_id, "WebSocket connected");

    // Validate JWT if provided (resolve Gatekeeper user ID)
    let gatekeeper_user_id = if let Some(ref jwt_cache) = state.jwt_cache {
        if let Some(ref token) = token {
            match jwt_cache.validate(token).await {
                Ok(claims) => {
                    tracing::info!(user_id = %claims.sub, "authenticated via Gatekeeper JWT");
                    Some(claims.sub)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "JWT validation failed, using session identity");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Create channel for sending messages back to client
    let (tx, mut rx) = mpsc::channel::<WsOutgoing>(32);

    // Spawn task to forward messages from channel to WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming messages
    let session_id_clone = session_id.clone();
    let gk_user_clone = gatekeeper_user_id.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if let Err(e) =
                        handle_message(&text, &state, &session_id_clone, tx.clone(), gk_user_clone.as_deref()).await
                    {
                        let error = WsOutgoing::Error {
                            code: "internal_error".to_string(),
                            message: e.to_string(),
                        };
                        let _ = tx.send(error).await;
                    }
                }
                Message::Ping(data) => {
                    // axum handles pong automatically, but we can log it
                    tracing::trace!(len = data.len(), "received ping");
                }
                Message::Close(_) => {
                    tracing::info!(session_id = %session_id_clone, "WebSocket closed by client");
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    tracing::info!(session_id = %session_id, "WebSocket disconnected");
}

/// Handle a single incoming message
async fn handle_message(
    text: &str,
    state: &Arc<ApiState>,
    session_id: &str,
    tx: mpsc::Sender<WsOutgoing>,
    gatekeeper_user_id: Option<&str>,
) -> crate::Result<()> {
    let incoming: WsIncoming = serde_json::from_str(text)
        .map_err(|e| crate::Error::Config(format!("invalid message: {e}")))?;

    match incoming {
        WsIncoming::Ping => {
            tx.send(WsOutgoing::Pong)
                .await
                .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
        }
        WsIncoming::Chat { content } => {
            handle_chat_message(&content, state, session_id, tx, gatekeeper_user_id).await?;
        }
    }

    Ok(())
}

/// Handle a chat message and stream the response
#[allow(clippy::too_many_lines)]
async fn handle_chat_message(
    content: &str,
    state: &Arc<ApiState>,
    session_id: &str,
    tx: mpsc::Sender<WsOutgoing>,
    gatekeeper_user_id: Option<&str>,
) -> crate::Result<()> {
    // Use Gatekeeper identity if available, otherwise derive from session_id
    let user_id = if let Some(gk_id) = gatekeeper_user_id {
        gk_id.to_string()
    } else if let Some(suffix) = session_id.strip_prefix("web-") {
        format!("web-user-{suffix}")
    } else {
        session_id.to_string()
    };

    // Ensure user exists (creates if not found)
    let user = state
        .user_repo
        .find_or_create(&user_id)
        .map_err(|e| crate::Error::Database(e.to_string()))?;

    // Build context with memory (use active persona for dynamic switching)
    let active = state.active_persona.read().await;
    let active_persona_id = active.id.clone();
    let active_system_prompt = active.system_prompt.clone();
    drop(active); // Release lock before database operations

    // Ensure session exists (creates if not found)
    // For web clients, use session_id as channel_id
    let session = state
        .session_repo
        .find_or_create(&user_id, "web", session_id, &active_persona_id)
        .map_err(|e| crate::Error::Database(e.to_string()))?;

    // Store user message (use session.id, not the client's session_id)
    state
        .session_repo
        .add_message(&session.id, MessageRole::User, content)?;

    // Build context config with active persona
    let context_config = crate::api::ApiServer::context_config(
        &active_persona_id,
        active_system_prompt,
    );
    let context_builder = ContextBuilder::new(context_config);
    let mut built_context = context_builder.build_with_memory(
        &session.id,
        &user_id,
        user.life_json_path.as_deref(),
        &state.session_repo,
        &state.user_repo,
        Some(&state.memory_repo),
    );

    // Inject knowledge based on user message
    if let Ok(ref mut ctx) = built_context {
        if !state.persona_knowledge.is_empty() {
            let max_knowledge_tokens = state.max_context_tokens / 4;
            let selected = crate::knowledge::select_knowledge(
                &state.persona_knowledge,
                content,
                max_knowledge_tokens,
            );
            if !selected.is_empty() {
                ctx.knowledge_context = crate::knowledge::format_knowledge(&selected);
            }
        }
    }

    // Build augmented prompt
    let augmented_prompt = built_context
        .as_ref()
        .map_or_else(|_| content.to_string(), |ctx| ctx.format_prompt(content));

    // Resolve provider for this user
    let agent_to_use = if let (Some(gk_user_id), Some(resolver)) =
        (&gatekeeper_user_id, &state.key_resolver)
    {
        // Try per-user key resolution
        match resolve_user_agent(resolver, gk_user_id, state).await {
            Some(agent) => Some(agent),
            None => state.agent.clone(), // Fall back to default agent
        }
    } else {
        state.agent.clone()
    };

    let Some(agent) = agent_to_use else {
        let error = WsOutgoing::Error {
            code: "no_agent".to_string(),
            message: "No LLM provider configured. Add your API key in settings".to_string(),
        };
        tx.send(error)
            .await
            .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
        return Ok(());
    };

    // Process with agent (stream chunks)
    let response = {
        let mut agent_guard = agent.lock().await;
        agent_guard.clear();

        // Apply tool filter based on web channel policy
        let allowed_tools = state.tool_policy.allowed_tools("web");
        agent_guard.set_tool_filter(Some(allowed_tools));

        let tx_clone = tx.clone();
        match agent_guard
            .chat(&augmented_prompt, move |chunk| {
                let msg = WsOutgoing::ChatChunk {
                    content: chunk.to_string(),
                };
                // Use try_send to maintain chunk order (sync, non-blocking)
                let _ = tx_clone.try_send(msg);
            })
            .await
        {
            Ok(response) => response,
            Err(e) => {
                let error = WsOutgoing::Error {
                    code: "agent_error".to_string(),
                    message: e.to_string(),
                };
                tx.send(error)
                    .await
                    .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
                return Ok(());
            }
        }
    };

    // Store assistant response
    let message = state
        .session_repo
        .add_message(&session.id, MessageRole::Assistant, &response)?;

    // Send completion message
    let complete = WsOutgoing::ChatComplete {
        message_id: message.id,
    };
    tx.send(complete)
        .await
        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;

    Ok(())
}

/// Resolve or create an agent for a specific user based on their BYOK key
async fn resolve_user_agent(
    resolver: &crate::providers::KeyResolver,
    user_id: &str,
    state: &Arc<ApiState>,
) -> Option<Arc<Mutex<Agent>>> {
    // Try Anthropic first, then OpenAI, then OpenRouter
    for provider_name in &["anthropic", "openai", "openrouter"] {
        if let Ok(Some(resolved)) = resolver.resolve(user_id, provider_name).await {
            if !resolved.is_user_key {
                continue; // Skip env fallbacks here, handled by state.agent
            }

            let model = resolved.model_override.as_deref().unwrap_or(
                match *provider_name {
                    "anthropic" => crate::daemon::DEFAULT_ANTHROPIC_MODEL,
                    _ => crate::daemon::DEFAULT_OPENAI_MODEL,
                },
            );

            let provider_box: Option<Box<dyn omni_cli::core::agent::LlmProvider>> =
                match *provider_name {
                    "anthropic" => {
                        omni_cli::core::agent::providers::AnthropicProvider::new(resolved.api_key)
                            .ok()
                            .map(|p| Box::new(p) as _)
                    }
                    "openai" | "openrouter" => {
                        omni_cli::core::agent::providers::OpenAiProvider::new(resolved.api_key)
                            .ok()
                            .map(|p| Box::new(p) as _)
                    }
                    _ => None,
                };

            if let Some(provider) = provider_box {
                let system_prompt = {
                    let active = state.active_persona.read().await;
                    active.system_prompt.clone()
                };

                let agent = Arc::new(Mutex::new(Agent::with_system(
                    provider,
                    model,
                    1024,
                    system_prompt.unwrap_or_default(),
                )));

                return Some(agent);
            }
        }
    }

    None
}
