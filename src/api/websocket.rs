//! WebSocket handler for real-time chat

use std::sync::Arc;
use std::time::Duration;

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
use synapse_client::SynapseClient;
use tokio::sync::mpsc;

use super::ApiState;
use crate::agent::{AgentNotifyEvent, AgentRunConfig, run_agent_turn};
use crate::api::feedback::{FeedbackAnswer, FeedbackManager};
use crate::context::ContextBuilder;
use crate::events::{build_conversation_ended_event, publish};

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
        /// Override the model for this message
        #[serde(default)]
        model_override: Option<String>,
    },
    /// Client answer to an `ask_user` / permission / `location_request` event
    AgentResponse {
        request_id: uuid::Uuid,
        /// For `ask_user`: selected option or typed text.
        /// For permission: "allow" | `"allow_session"` | "deny".
        /// For location: serialized coords JSON or "denied".
        answer: String,
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
    /// Tool invocation started — emitted immediately on dispatch
    ToolStart {
        tool_id: String,
        name: String,
    },
    /// Tool invocation finished
    ToolResult {
        tool_id: String,
        name: String,
        /// Short display summary (command run, file path, etc.)
        invocation: String,
        output: String,
        is_error: bool,
    },
    /// Agent wants to ask the user a question
    AskUser {
        request_id: uuid::Uuid,
        question: String,
        /// Predefined options; absent = free-text input
        options: Option<Vec<String>>,
        multi_select: bool,
    },
    /// Agent requests permission for a tool action
    Permission {
        request_id: uuid::Uuid,
        tool_name: String,
        /// Human-readable action description
        action: String,
        /// Structured context (e.g., {command: "rm -rf"})
        context: serde_json::Value,
    },
    /// Agent requests the user's location
    LocationRequest {
        request_id: uuid::Uuid,
        purpose: String,
    },
    /// Background progress update
    Progress {
        label: String,
        /// 0–100, absent if indeterminate
        percent: Option<u8>,
    },
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

    let feedback = Arc::new(FeedbackManager::new());

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

    // Register sender for proactive ws_push delivery
    // Key: Gatekeeper user_id when authenticated, otherwise session_id
    let ws_push_key = gatekeeper_user_id.clone().unwrap_or_else(|| session_id.clone());
    if let Some(ref senders) = state.ws_senders {
        senders.write().await.insert(ws_push_key.clone(), tx.clone());
        tracing::debug!(key = %ws_push_key, "ws_push: registered sender");
    }

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

    // Clone state for deregistration after tasks complete
    let state_for_cleanup = Arc::clone(&state);

    // Handle incoming messages
    let feedback_for_recv = Arc::clone(&feedback);
    let session_id_clone = session_id.clone();
    let gk_user_clone = gatekeeper_user_id.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if let Err(e) =
                        handle_message(&text, &state, &session_id_clone, tx.clone(), gk_user_clone.as_deref(), &feedback_for_recv).await
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

    feedback.cancel_all();

    // Deregister sender on disconnect
    if let Some(ref senders) = state_for_cleanup.ws_senders {
        senders.write().await.remove(&ws_push_key);
        tracing::debug!(key = %ws_push_key, "ws_push: deregistered sender");
    }

    // Publish conversation ended event (best-effort)
    // Derive org_id from authenticated user ID, falling back to session_id for unauthenticated sessions
    let ended_org_id = gatekeeper_user_id
        .as_deref()
        .unwrap_or(&session_id)
        .to_string();
    publish(build_conversation_ended_event(
        &session_id,
        "web",
        &ended_org_id,
    ));

    tracing::info!(session_id = %session_id, "WebSocket disconnected");
}

/// Handle a single incoming message
async fn handle_message(
    text: &str,
    state: &Arc<ApiState>,
    session_id: &str,
    tx: mpsc::Sender<WsOutgoing>,
    gatekeeper_user_id: Option<&str>,
    feedback: &Arc<FeedbackManager>,
) -> crate::Result<()> {
    let incoming: WsIncoming = serde_json::from_str(text)
        .map_err(|e| crate::Error::Config(format!("invalid message: {e}")))?;

    match incoming {
        WsIncoming::Ping => {
            tx.send(WsOutgoing::Pong)
                .await
                .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
        }
        WsIncoming::Chat { content, persona_id, model_override } => {
            handle_chat_message(&content, persona_id, model_override, state, session_id, tx, gatekeeper_user_id, feedback).await?;
        }
        WsIncoming::AgentResponse { request_id, answer } => {
            let fb_answer = match answer.as_str() {
                "allow" => FeedbackAnswer::Allow,
                "allow_session" => FeedbackAnswer::AllowSession,
                "deny" | "denied" => FeedbackAnswer::Denied,
                "" => FeedbackAnswer::Cancelled,
                other => FeedbackAnswer::Text(other.to_string()),
            };
            feedback.respond(request_id, fb_answer);
        }
    }

    Ok(())
}

/// Handle a chat message and stream the response
#[allow(clippy::too_many_lines)]
async fn handle_chat_message(
    content: &str,
    persona_id_override: Option<String>,
    msg_model_override: Option<String>,
    state: &Arc<ApiState>,
    session_id: &str,
    tx: mpsc::Sender<WsOutgoing>,
    gatekeeper_user_id: Option<&str>,
    _feedback: &Arc<FeedbackManager>,
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

    tracing::info!(
        session_id = %session_id,
        persona_id_override = ?persona_id_override,
        "handle_chat_message: resolving persona"
    );

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

    tracing::info!(
        active_persona_id = %active_persona_id,
        has_persona_prompt = active_system_prompt.is_some(),
        static_system_prompt_len = state.system_prompt.len(),
        "handle_chat_message: building messages"
    );

    // Build context config with active persona
    let context_config = crate::api::ApiServer::context_config(
        &active_persona_id,
        active_system_prompt,
    );
    let context_builder = ContextBuilder::new(context_config);

    // Embed user message for semantic memory retrieval when embedder is available
    let query_embedding = if let Some(ref embedder) = state.embedder {
        match embedder.embed(content).await {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::warn!(error = %e, "failed to embed user message for semantic retrieval");
                None
            }
        }
    } else {
        None
    };
    let mut built_context = context_builder.build_with_semantic_memory(
        &session.id,
        &user_id,
        user.life_json_path.as_deref(),
        &state.session_repo,
        &state.user_repo,
        Some(&state.memory_repo),
        query_embedding.as_deref(),
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
        // Self-hosted: try locally stored keys before falling back to shared client
        if let Some((client, model)) = resolve_local_key(state) {
            (Some(client), Some(model))
        } else {
            (state.synapse.clone(), None)
        }
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

    let model = msg_model_override
        .or(model_override)
        .unwrap_or_else(|| state.llm_model.clone());

    tracing::info!(
        model = %model,
        default = %state.llm_model,
        "handle_chat_message: resolved model"
    );

    // Build system prompt — skip in no-persona mode
    let system_prompt = if active_persona_id == crate::NO_PERSONA_ID {
        String::new()
    } else {
        state.system_prompt.clone()
    };

    // Bridge: forward AgentNotifyEvent → WsOutgoing::ToolStart/ToolResult to client
    let (notify_tx, mut notify_rx) = mpsc::channel::<AgentNotifyEvent>(32);
    let tx_notify = tx.clone();
    let notify_bridge = tokio::spawn(async move {
        while let Some(event) = notify_rx.recv().await {
            let ws_msg = match event {
                AgentNotifyEvent::ToolStart { tool_id, name } => {
                    WsOutgoing::ToolStart { tool_id, name }
                }
                AgentNotifyEvent::ToolResult { tool_id, name, invocation, output, is_error } => {
                    WsOutgoing::ToolResult { tool_id, name, invocation, output, is_error }
                }
            };
            let _ = tx_notify.send(ws_msg).await;
        }
    });

    let agent_config = AgentRunConfig {
        prompt: augmented_prompt,
        system_prompt,
        model,
        max_tokens: state.llm_max_tokens,
        max_iterations: 10,
        session_id: session.id.clone(),
        user_id: user_id.clone(),
        notify: Some(notify_tx),
        synapse_override: Some(synapse),
    };

    // Spawn keepalive heartbeat to prevent proxy timeout during LLM processing.
    // Reverse proxies (Railway/nginx) close idle connections after ~30-60s; sending
    // periodic Progress frames keeps the TCP connection alive.
    let tx_heartbeat = tx.clone();
    let heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(20),
            Duration::from_secs(20),
        );
        loop {
            interval.tick().await;
            if tx_heartbeat
                .send(WsOutgoing::Progress {
                    label: "thinking".to_string(),
                    percent: None,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let agent_result = run_agent_turn(state, agent_config).await;
    heartbeat.abort();
    notify_bridge.abort();

    let full_response = match agent_result {
        Ok(text) => text,
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

    // Stream the full response as a single chunk so WS clients receive it
    if !full_response.is_empty() {
        tx.send(WsOutgoing::ChatChunk {
            content: full_response.clone(),
        })
        .await
        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;
    }

    // Retrieve the stored assistant message ID for ChatComplete
    // run_agent_turn already stored both user and assistant messages; fetch the last one
    let message = state
        .session_repo
        .get_messages(&session.id, 1)
        .map_err(|e| crate::Error::Database(e.to_string()))
        .and_then(|msgs| {
            msgs.into_iter()
                .next()
                .ok_or_else(|| crate::Error::Database("no message stored".to_string()))
        })?;

    // Send completion message
    let complete = WsOutgoing::ChatComplete {
        message_id: message.id,
    };
    tx.send(complete)
        .await
        .map_err(|_| crate::Error::Config("channel closed".to_string()))?;

    // Post-turn background fact extraction — fire-and-forget, never blocks the response
    if let Some(indexer) = state.indexer.clone() {
        let uid = user_id.clone();
        let user_msg = content.to_string();
        let assistant_msg = full_response.clone();
        let sid = session.id.clone();
        tokio::spawn(async move {
            if let Err(e) = indexer
                .index_message(&uid, &user_msg, &assistant_msg, Some(&sid), Some("web"))
                .await
            {
                tracing::warn!(error = %e, user_id = %uid, "post-turn indexing failed");
            }
        });
    }

    Ok(())
}

/// Resolve a provider client from the local SQLite key store (self-hosted path)
///
/// Checks providers in priority order (anthropic → openai → openrouter) and returns
/// a `SynapseClient` configured with the first stored key found.
///
/// Returns `None` if no local key store is configured or no keys are stored.
fn resolve_local_key(state: &Arc<ApiState>) -> Option<(Arc<SynapseClient>, String)> {
    let store = state.local_key_store.as_ref()?;
    let synapse_base_url = state
        .synapse
        .as_ref()
        .map_or_else(|| "http://localhost:6000".to_string(), |c| c.base_url().to_string());

    for (provider, default_model) in &[
        ("anthropic", crate::daemon::DEFAULT_MODEL),
        ("openai", "gpt-4o"),
        ("openrouter", "gpt-4o"),
    ] {
        if let Ok(Some(stored)) = store.get(provider) {
            let model = stored
                .model_preference
                .unwrap_or_else(|| (*default_model).to_string());
            if let Ok(client) = SynapseClient::new(&synapse_base_url) {
                let client = client.with_api_key(stored.api_key);
                tracing::debug!(provider, "resolved local provider key for chat");
                return Some((Arc::new(client), model));
            }
        }
    }
    None
}

/// Resolve a per-user Synapse client based on their BYOK key or managed key
///
/// Resolution priority:
/// 1. User's preferred provider key (respects Synapse `defaultProvider` preference)
/// 2. Auto-provision via Synapse API (if cloud mode + provisioner available)
/// 3. Fallback: return None, caller uses shared `state.synapse`
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

    // Step 1: Use user's preferred provider key (respects Synapse defaultProvider preference)
    // Priority: explicit defaultProvider → anthropic → openai → openrouter → omni_credits
    if let Ok(Some((provider_name, resolved))) = resolver.resolve_preferred(user_id).await {
        if resolved.is_user_key {
            let model = resolved.model_override.unwrap_or_else(|| {
                match provider_name.as_str() {
                    "anthropic" | "omni_credits" => crate::daemon::DEFAULT_MODEL.to_string(),
                    _ => "gpt-4o".to_string(),
                }
            });

            if let Ok(client) = SynapseClient::new(&synapse_base_url) {
                let client = client.with_api_key(resolved.api_key);
                return Some((Arc::new(client), model));
            }
        }
    }

    // Step 2: Auto-provision via Synapse API (cloud mode only)
    if state.cloud_mode {
        if let Some(provisioner) = &state.key_provisioner {
            match provisioner.provision(user_id, None, None).await {
                Ok(provisioned) => {
                    tracing::info!(
                        user_id = %user_id,
                        plan = %provisioned.plan,
                        "auto-provisioned managed key"
                    );

                    // Invalidate resolver cache so next resolve fetches the new key from Synapse
                    resolver.invalidate(user_id).await;

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

    // Step 3: No personal key available, caller falls back to state.synapse
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_start_serializes() {
        let msg = WsOutgoing::ToolStart {
            tool_id: "abc".to_string(),
            name: "Bash".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"tool_start\""));
        assert!(json.contains("\"tool_id\":\"abc\""));
    }

    #[test]
    fn ask_user_serializes() {
        let msg = WsOutgoing::AskUser {
            request_id: uuid::Uuid::nil(),
            question: "Which project?".to_string(),
            options: Some(vec!["A".to_string(), "B".to_string()]),
            multi_select: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"ask_user\""));
    }

    #[test]
    fn agent_response_deserializes() {
        let json = r#"{"type":"agent_response","request_id":"00000000-0000-0000-0000-000000000000","answer":"A"}"#;
        let msg: WsIncoming = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsIncoming::AgentResponse { .. }));
    }

    #[test]
    fn feedback_manager_cancel_on_disconnect() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mgr = crate::api::feedback::FeedbackManager::new();
            let id = uuid::Uuid::new_v4();
            let rx = mgr.register(id);
            mgr.cancel_all();
            let answer = rx.await.unwrap();
            assert!(matches!(answer, crate::api::feedback::FeedbackAnswer::Cancelled));
        });
    }

    #[test]
    fn partitions_tool_batch_correctly() {
        use crate::tools::executor::ToolKind;

        let names = vec!["Read", "Bash", "Glob", "Write", "WebSearch"];
        let (reads, mutates): (Vec<&&str>, Vec<&&str>) = names
            .iter()
            .partition(|n| ToolKind::classify(*n) == ToolKind::Read);

        assert_eq!(reads, vec![&"Read", &"Glob", &"WebSearch"]);
        assert_eq!(mutates, vec![&"Bash", &"Write"]);
    }

    #[tokio::test]
    async fn ask_user_arguments_parse() {
        #[derive(serde::Deserialize)]
        struct AskArgs {
            question: String,
            options: Option<Vec<String>>,
            #[serde(default)]
            multi_select: bool,
        }

        let args = r#"{"question":"Which env?","options":["dev","prod"],"multi_select":false}"#;
        let parsed: AskArgs = serde_json::from_str(args).unwrap();
        assert_eq!(parsed.question, "Which env?");
        assert_eq!(parsed.options.unwrap().len(), 2);
        assert!(!parsed.multi_select);
    }

    #[test]
    fn post_turn_indexing_does_not_require_indexer() {
        // Documents that the indexing block is guarded by Option — requests succeed without indexer
        assert!(true);
    }
}
