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
use serde::{Deserialize, Serialize};
use synapse_client::{ChatEvent, SynapseClient};
use tokio::sync::mpsc;

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
    Chat {
        content: String,
        /// Override the active persona for this message
        #[serde(default)]
        persona_id: Option<String>,
    },
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

    // Cloud mode: require valid JWT
    if state.cloud_mode && gatekeeper_user_id.is_none() {
        tracing::warn!(session_id = %session_id, "cloud mode: rejecting unauthenticated WebSocket");
        let error = WsOutgoing::Error {
            code: "auth_required".to_string(),
            message: "Authentication required. Please sign in.".to_string(),
        };
        if let Ok(msg) = serde_json::to_string(&error) {
            let _ = sender.send(Message::Text(msg.into())).await;
        }
        return;
    }

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
        WsIncoming::Chat { content, persona_id } => {
            handle_chat_message(&content, persona_id, state, session_id, tx, gatekeeper_user_id).await?;
        }
    }

    Ok(())
}

/// Handle a chat message and stream the response
#[allow(clippy::too_many_lines)]
async fn handle_chat_message(
    content: &str,
    persona_id_override: Option<String>,
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

    // Resolve persona: prefer per-message override, fall back to active persona
    let (active_persona_id, active_system_prompt) = if let Some(ref override_id) = persona_id_override {
        // No-persona mode: skip system prompt entirely
        if override_id == crate::NO_PERSONA_ID {
            (override_id.clone(), None)
        // Load the requested persona from cache or embedded defaults
        } else if let Some((_info, system_prompt)) = super::health::load_full_persona(&state.persona_cache_dir, override_id)
            .or_else(|| {
                crate::Config::load_embedded_persona(override_id).ok().map(|p| {
                    let prompt = p.system_prompt().map(String::from);
                    (super::health::persona_to_info(&p), prompt)
                })
            })
        {
            (override_id.clone(), system_prompt)
        } else {
            tracing::warn!(persona_id = %override_id, "requested persona not found, using active");
            let active = state.active_persona.read().await;
            let id = active.id.clone();
            let prompt = active.system_prompt.clone();
            drop(active);
            (id, prompt)
        }
    } else {
        let active = state.active_persona.read().await;
        let id = active.id.clone();
        let prompt = active.system_prompt.clone();
        drop(active);
        (id, prompt)
    };

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

    // Resolve Synapse client for this user (BYOK or default)
    let (synapse_to_use, model_override) = if let (Some(gk_user_id), Some(resolver)) =
        (&gatekeeper_user_id, &state.key_resolver)
    {
        match resolve_user_synapse(resolver, gk_user_id, state).await {
            Some((client, model)) => (Some(client), Some(model)),
            None if state.cloud_mode => {
                tracing::warn!(user_id = %gk_user_id, "cloud mode: no user key resolved, cannot fall back to shared client");
                let error = WsOutgoing::Error {
                    code: "provision_failed".to_string(),
                    message: "Unable to provision your account. Please try again or check your API key settings.".to_string(),
                };
                tx.send(error)
                    .await
                    .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
                return Ok(());
            }
            None => (state.synapse.clone(), None),
        }
    } else if state.cloud_mode {
        tracing::warn!("cloud mode: no key resolver configured, cannot resolve user keys");
        let error = WsOutgoing::Error {
            code: "config_error".to_string(),
            message: "Service misconfigured. Please contact support.".to_string(),
        };
        tx.send(error)
            .await
            .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
        return Ok(());
    } else {
        (state.synapse.clone(), None)
    };

    let Some(synapse) = synapse_to_use else {
        let error = WsOutgoing::Error {
            code: "no_agent".to_string(),
            message: "No LLM provider configured. Add your API key in settings".to_string(),
        };
        tx.send(error)
            .await
            .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
        return Ok(());
    };

    let model = model_override.unwrap_or_else(|| state.llm_model.clone());

    // Fetch MCP tools from Synapse and plugins if available
    let tools = if let Some(ref synapse) = state.synapse {
        let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(synapse), state.plugin_manager.clone());
        executor.list_tools().await.ok()
    } else {
        None
    };

    // Build initial messages — skip system prompt in no-persona mode
    let mut messages = if active_persona_id == crate::NO_PERSONA_ID {
        vec![synapse_client::Message::user(&augmented_prompt)]
    } else {
        vec![
            synapse_client::Message::system(&state.system_prompt),
            synapse_client::Message::user(&augmented_prompt),
        ]
    };

    // Multi-turn tool loop (max 10 rounds to prevent runaway)
    let mut full_response = String::new();
    for _turn in 0..10 {
        let request = synapse_client::ChatRequest {
            model: model.clone(),
            messages: messages.clone(),
            stream: true,
            temperature: None,
            top_p: None,
            max_tokens: Some(state.llm_max_tokens),
            stop: None,
            tools: tools.clone(),
            tool_choice: None,
        };

        let mut stream = match synapse.chat_completion_stream(&request).await {
            Ok(s) => s,
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
        };

        // Accumulate tool calls from the stream
        let mut turn_text = String::new();
        let mut pending_tool_calls: Vec<PendingToolCall> = Vec::new();
        let mut finish_reason = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(ChatEvent::ContentDelta(text)) => {
                    turn_text.push_str(&text);
                    let msg = WsOutgoing::ChatChunk { content: text };
                    let _ = tx.try_send(msg);
                }
                Ok(ChatEvent::ToolCallStart { index, id, name }) => {
                    let idx = index as usize;
                    if idx >= pending_tool_calls.len() {
                        pending_tool_calls.resize_with(idx + 1, PendingToolCall::default);
                    }
                    pending_tool_calls[idx].id = id;
                    pending_tool_calls[idx].name = name;
                }
                Ok(ChatEvent::ToolCallDelta { index, arguments }) => {
                    let idx = index as usize;
                    if idx < pending_tool_calls.len() {
                        pending_tool_calls[idx].arguments.push_str(&arguments);
                    }
                }
                Ok(ChatEvent::Done { finish_reason: fr, .. }) => {
                    finish_reason = fr;
                    break;
                }
                Ok(ChatEvent::Error(e)) => {
                    let error = WsOutgoing::Error {
                        code: "agent_error".to_string(),
                        message: e,
                    };
                    tx.send(error)
                        .await
                        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
                    return Ok(());
                }
                Err(e) => {
                    let error = WsOutgoing::Error {
                        code: "stream_error".to_string(),
                        message: e.to_string(),
                    };
                    tx.send(error)
                        .await
                        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
                    return Ok(());
                }
            }
        }

        full_response.push_str(&turn_text);

        // If the model requested tool calls, execute them and loop
        if finish_reason.as_deref() == Some("tool_calls") && !pending_tool_calls.is_empty() {
            // Add assistant message with tool calls to conversation
            let tool_calls: Vec<synapse_client::ToolCall> = pending_tool_calls
                .iter()
                .map(|tc| synapse_client::ToolCall {
                    id: tc.id.clone(),
                    function: synapse_client::FunctionCall {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect();

            let assistant_content = if turn_text.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(turn_text)
            };

            messages.push(synapse_client::Message {
                role: "assistant".to_owned(),
                content: assistant_content,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
            });

            // Execute each tool call
            let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(&synapse), state.plugin_manager.clone());
            for tc in &pending_tool_calls {
                let result = executor
                    .execute(&tc.name, &tc.arguments)
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}"));

                messages.push(synapse_client::Message::tool(&tc.id, &result));
            }

            continue; // Next turn
        }

        break; // No tool calls, done
    }

    // Store assistant response
    let message = state
        .session_repo
        .add_message(&session.id, MessageRole::Assistant, &full_response)?;

    // Send completion message
    let complete = WsOutgoing::ChatComplete {
        message_id: message.id,
    };
    tx.send(complete)
        .await
        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;

    Ok(())
}

/// In-progress tool call being assembled from streaming events
#[derive(Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Resolve a per-user Synapse client based on their BYOK key or managed key
///
/// Resolution priority:
/// 1. BYOK keys (anthropic, openai, openrouter from vault)
/// 2. Cached Omni Credits key (from vault)
/// 3. Auto-provision via Synapse API (if cloud mode + provisioner available)
/// 4. Fallback: return None, caller uses shared `state.synapse`
async fn resolve_user_synapse(
    resolver: &crate::providers::KeyResolver,
    user_id: &str,
    state: &Arc<ApiState>,
) -> Option<(Arc<SynapseClient>, String)> {
    let synapse_base_url = state
        .synapse
        .as_ref()
        .map_or_else(
            || "http://localhost:6000".to_string(),
            |c| c.base_url().to_string(),
        );

    // Step 1: Try BYOK keys (Anthropic, OpenAI, OpenRouter)
    for provider_name in &["anthropic", "openai", "openrouter"] {
        if let Ok(Some(resolved)) = resolver.resolve(user_id, provider_name).await {
            if !resolved.is_user_key {
                continue; // Skip env fallbacks, handled by state.synapse
            }

            let model = resolved.model_override.unwrap_or_else(|| {
                match *provider_name {
                    "anthropic" => crate::daemon::DEFAULT_MODEL.to_string(),
                    _ => "gpt-4o".to_string(),
                }
            });

            if let Ok(client) = SynapseClient::new(&synapse_base_url) {
                let client = client.with_api_key(resolved.api_key);
                return Some((Arc::new(client), model));
            }
        }
    }

    // Step 2: Try cached Omni Credits key from vault
    if let Ok(Some(resolved)) = resolver.resolve(user_id, "omni_credits").await {
        if resolved.is_user_key {
            let model = crate::daemon::DEFAULT_MODEL.to_string();

            if let Ok(client) = SynapseClient::new(&synapse_base_url) {
                let client = client.with_api_key(resolved.api_key);
                return Some((Arc::new(client), model));
            }
        }
    }

    // Step 3: Auto-provision via Synapse API (cloud mode only)
    if state.cloud_mode {
        if let Some(provisioner) = &state.key_provisioner {
            match provisioner.provision(user_id, None, None).await {
                Ok(provisioned) => {
                    tracing::info!(
                        user_id = %user_id,
                        plan = %provisioned.plan,
                        "auto-provisioned managed key"
                    );

                    // Cache the provisioned key in Gatekeeper vault
                    if let Err(e) = resolver
                        .store(user_id, "omni_credits", &provisioned.api_key, None)
                        .await
                    {
                        tracing::warn!(error = %e, "failed to cache provisioned key in vault");
                    }

                    let model = crate::daemon::DEFAULT_MODEL.to_string();

                    if let Ok(client) = SynapseClient::new(&synapse_base_url) {
                        let client = client.with_api_key(provisioned.api_key);
                        return Some((Arc::new(client), model));
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        user_id = %user_id,
                        "auto-provision failed"
                    );
                }
            }
        }
    }

    // Step 4: No personal key available, caller falls back to state.synapse
    None
}
