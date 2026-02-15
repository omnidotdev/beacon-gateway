//! Node registry API endpoints
//!
//! WebSocket endpoint for node connections and REST endpoints
//! for listing nodes and invoking commands

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::nodes::{InvokeResult, NodeRegistration, NodeRegistry, NodeSession};
use crate::nodes::policy::is_command_allowed;

/// Shared node registry state
pub type SharedNodeRegistry = Arc<Mutex<NodeRegistry>>;

/// Query parameters for node WebSocket connection
#[derive(Debug, Deserialize)]
struct NodeWsQuery {
    device_id: Option<String>,
}

/// Outgoing message from gateway to node
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum GatewayToNode {
    /// Registration accepted
    Registered { node_id: String },
    /// Invoke a command on the node
    Invoke {
        correlation_id: String,
        command: String,
        params: serde_json::Value,
    },
    /// Error message
    Error { code: String, message: String },
}

/// Incoming message from node to gateway
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NodeToGateway {
    /// Registration message
    Register(NodeRegistration),
    /// Response to an invocation
    InvokeResponse {
        correlation_id: String,
        ok: bool,
        payload: Option<serde_json::Value>,
        error: Option<String>,
    },
    /// Keepalive ping
    Ping,
}

/// REST response for listing nodes
#[derive(Serialize)]
pub struct NodeResponse {
    pub node_id: String,
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub device_family: Option<String>,
    pub caps: Vec<String>,
    pub commands: Vec<String>,
    pub connected_at: String,
}

impl From<&NodeSession> for NodeResponse {
    fn from(session: &NodeSession) -> Self {
        Self {
            node_id: session.node_id.clone(),
            device_id: session.device_id.clone(),
            display_name: session.display_name.clone(),
            platform: session.platform.clone(),
            device_family: session.device_family.clone(),
            caps: session.caps.clone(),
            commands: session.commands.clone(),
            connected_at: session.connected_at.to_rfc3339(),
        }
    }
}

/// REST request for invoking a command
#[derive(Deserialize)]
pub struct InvokeBody {
    pub command: String,
    pub params: serde_json::Value,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub idempotency_key: Option<String>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

/// REST response for invoke result
#[derive(Serialize)]
pub struct InvokeResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build node routes
pub fn router(registry: SharedNodeRegistry) -> Router {
    Router::new()
        .route("/", get(list_nodes))
        .route("/{node_id}", get(get_node))
        .route("/{node_id}/invoke", post(invoke_node))
        .with_state(registry)
}

/// Build node WebSocket router
pub fn ws_router(registry: SharedNodeRegistry) -> Router {
    Router::new()
        .route("/node", get(ws_upgrade))
        .with_state(registry)
}

/// Handle WebSocket upgrade for node connections
async fn ws_upgrade(
    State(registry): State<SharedNodeRegistry>,
    query: Query<NodeWsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let device_id = query.0.device_id;
    ws.on_upgrade(move |socket| handle_node_socket(socket, registry, device_id))
}

/// Handle a connected node WebSocket
async fn handle_node_socket(
    socket: WebSocket,
    registry: SharedNodeRegistry,
    device_id: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut node_id: Option<String> = None;

    tracing::info!(device_id = ?device_id, "node WebSocket connected, awaiting registration");

    // Wait for registration message
    while let Some(Ok(msg)) = receiver.next().await {
        let Message::Text(text) = msg else {
            continue;
        };

        let Ok(incoming) = serde_json::from_str::<NodeToGateway>(&text) else {
            let err = GatewayToNode::Error {
                code: "invalid_message".to_string(),
                message: "expected registration message".to_string(),
            };
            if let Ok(json) = serde_json::to_string(&err) {
                let _ = sender.send(Message::Text(json.into())).await;
            }
            continue;
        };

        match incoming {
            NodeToGateway::Register(registration) => {
                let mut reg = registry.lock().await;
                let id = reg.register(registration);
                node_id = Some(id.clone());

                let ack = GatewayToNode::Registered { node_id: id.clone() };
                if let Ok(json) = serde_json::to_string(&ack) {
                    if sender.send(Message::Text(json.into())).await.is_err() {
                        return;
                    }
                }

                tracing::info!(node_id = %id, "node registered");
                break;
            }
            NodeToGateway::Ping => continue,
            _ => {
                let err = GatewayToNode::Error {
                    code: "invalid_message".to_string(),
                    message: "must register before sending other messages".to_string(),
                };
                if let Ok(json) = serde_json::to_string(&err) {
                    let _ = sender.send(Message::Text(json.into())).await;
                }
            }
        }
    }

    let Some(node_id) = node_id else {
        tracing::warn!("node disconnected before registration");
        return;
    };

    // Handle ongoing messages (invoke responses)
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                let Ok(incoming) = serde_json::from_str::<NodeToGateway>(&text) else {
                    continue;
                };

                match incoming {
                    NodeToGateway::InvokeResponse {
                        correlation_id,
                        ok,
                        payload,
                        error,
                    } => {
                        let result = InvokeResult { ok, payload, error };
                        let mut reg = registry.lock().await;
                        if !reg.handle_response(&correlation_id, result) {
                            tracing::warn!(
                                correlation_id = %correlation_id,
                                "no pending invocation for correlation ID"
                            );
                        }
                    }
                    NodeToGateway::Ping => {}
                    NodeToGateway::Register(_) => {
                        tracing::warn!(node_id = %node_id, "duplicate registration ignored");
                    }
                }
            }
            Message::Ping(_) => {}
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup on disconnect
    let mut reg = registry.lock().await;
    reg.unregister(&node_id);
    tracing::info!(node_id = %node_id, "node disconnected and unregistered");
}

/// List all connected nodes
async fn list_nodes(
    State(registry): State<SharedNodeRegistry>,
) -> Json<Vec<NodeResponse>> {
    let reg = registry.lock().await;
    let nodes: Vec<NodeResponse> = reg.list().iter().map(|n| NodeResponse::from(*n)).collect();
    Json(nodes)
}

/// Get a specific node
async fn get_node(
    State(registry): State<SharedNodeRegistry>,
    Path(node_id): Path<String>,
) -> Result<Json<NodeResponse>, StatusCode> {
    let reg = registry.lock().await;
    reg.get(&node_id)
        .map(|n| Json(NodeResponse::from(n)))
        .ok_or(StatusCode::NOT_FOUND)
}

/// Invoke a command on a node
async fn invoke_node(
    State(registry): State<SharedNodeRegistry>,
    Path(node_id): Path<String>,
    Json(body): Json<InvokeBody>,
) -> Result<Json<InvokeResponse>, (StatusCode, Json<InvokeResponse>)> {
    let err = |code: StatusCode, msg: &str| {
        (
            code,
            Json(InvokeResponse {
                ok: false,
                payload: None,
                error: Some(msg.to_string()),
            }),
        )
    };

    // Check command policy
    let (platform, declared_commands) = {
        let reg = registry.lock().await;
        let node = reg.get(&node_id).ok_or_else(|| {
            err(StatusCode::NOT_FOUND, &format!("node '{node_id}' not found"))
        })?;
        (node.platform.clone(), node.commands.clone())
    };

    let deny_list = HashSet::new();
    if !is_command_allowed(&platform, &declared_commands, &deny_list, &body.command) {
        return Err(err(
            StatusCode::FORBIDDEN,
            &format!("command '{}' not allowed for platform '{platform}'", body.command),
        ));
    }

    // Prepare the invocation
    let (_correlation_id, rx) = {
        let mut reg = registry.lock().await;
        reg.prepare_invoke(&node_id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    };

    // TODO: send invoke message to node via WebSocket sender
    // For now, we wait for the response with a timeout
    let timeout = tokio::time::Duration::from_millis(body.timeout_ms);
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => Ok(Json(InvokeResponse {
            ok: result.ok,
            payload: result.payload,
            error: result.error,
        })),
        Ok(Err(_)) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invocation channel closed unexpectedly",
        )),
        Err(_) => Err(err(StatusCode::GATEWAY_TIMEOUT, "invocation timed out")),
    }
}
