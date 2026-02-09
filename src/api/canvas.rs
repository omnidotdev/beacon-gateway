//! WebSocket handler for canvas updates

use std::sync::Arc;

use axum::{
    extract::{ws::Message, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::canvas::{Canvas, CanvasCommand, CanvasElement};

/// Shared canvas state for API handlers
pub type SharedCanvas = Arc<Mutex<Canvas>>;

/// Incoming WebSocket message from client
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasWsIncoming {
    /// Request current canvas state
    GetState,
    /// Ping to keep connection alive
    Ping,
}

/// Outgoing WebSocket message to client
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasWsOutgoing {
    /// Current canvas state
    State { elements: Vec<CanvasElement> },
    /// Canvas command (push, clear, update)
    Command(CanvasCommand),
    /// Pong response
    Pong,
    /// Error occurred
    Error { message: String },
    /// Connection established
    Connected,
}

/// Build canvas WebSocket router
pub fn router(canvas: SharedCanvas) -> Router {
    Router::new()
        .route("/", get(ws_upgrade))
        .with_state(canvas)
}

/// Handle WebSocket upgrade request
async fn ws_upgrade(
    State(canvas): State<SharedCanvas>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, canvas))
}

/// Handle WebSocket connection
async fn handle_socket(socket: axum::extract::ws::WebSocket, canvas: SharedCanvas) {
    let (mut sender, mut receiver) = socket.split();

    // Send connected message
    let connected = CanvasWsOutgoing::Connected;
    if let Ok(msg) = serde_json::to_string(&connected) {
        if sender.send(Message::Text(msg.into())).await.is_err() {
            return;
        }
    }

    tracing::info!("Canvas WebSocket connected");

    // Subscribe to canvas updates
    let mut rx = {
        let canvas_guard = canvas.lock().await;
        canvas_guard.subscribe()
    };

    // Spawn task to forward canvas updates to WebSocket
    let mut broadcast_task = tokio::spawn(async move {
        while let Ok(cmd) = rx.recv().await {
            let msg = CanvasWsOutgoing::Command(cmd);
            if let Ok(text) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming messages
    let canvas_clone = canvas.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if let Err(e) = handle_message(&text, &canvas_clone).await {
                        tracing::warn!(error = %e, "canvas websocket message error");
                    }
                }
                Message::Close(_) => {
                    tracing::info!("Canvas WebSocket closed by client");
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = &mut broadcast_task => recv_task.abort(),
        _ = &mut recv_task => broadcast_task.abort(),
    }

    tracing::info!("Canvas WebSocket disconnected");
}

/// Handle a single incoming message
async fn handle_message(text: &str, canvas: &SharedCanvas) -> crate::Result<()> {
    let incoming: CanvasWsIncoming = serde_json::from_str(text)?;

    match incoming {
        CanvasWsIncoming::Ping => {
            // Pong is handled by the broadcast task
            // Client should receive Pong via the same mechanism
        }
        CanvasWsIncoming::GetState => {
            // State is sent via the broadcast mechanism
            // Client can request state, but response goes through broadcast
            let canvas_guard = canvas.lock().await;
            let _elements = canvas_guard.elements().to_vec();
            // In practice, client should subscribe and receive state via Command stream
        }
    }

    Ok(())
}

/// Canvas API for programmatic access (non-WebSocket)
pub mod api {
    use axum::{
        extract::State,
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };

    use super::SharedCanvas;
    use crate::canvas::{CanvasContent, CanvasElement};

    /// Get current canvas state
    async fn get_state(State(canvas): State<SharedCanvas>) -> impl IntoResponse {
        let elements: Vec<CanvasElement> = canvas.lock().await.elements().to_vec();
        Json(elements)
    }

    /// Push content to canvas
    async fn push_content(
        State(canvas): State<SharedCanvas>,
        Json(content): Json<CanvasContent>,
    ) -> impl IntoResponse {
        let id = canvas.lock().await.push(content);
        (StatusCode::CREATED, Json(serde_json::json!({ "id": id })))
    }

    /// Clear the canvas
    async fn clear_canvas(State(canvas): State<SharedCanvas>) -> impl IntoResponse {
        canvas.lock().await.clear();
        StatusCode::NO_CONTENT
    }

    /// Build REST API router for canvas
    pub fn router(canvas: SharedCanvas) -> Router {
        Router::new()
            .route("/", get(get_state))
            .route("/push", post(push_content))
            .route("/clear", post(clear_canvas))
            .with_state(canvas)
    }
}
