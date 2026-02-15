//! Plugin management REST endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::plugins::PluginManager;

/// Shared plugin manager state
pub type SharedPluginManager = Arc<Mutex<PluginManager>>;

/// Plugin info returned by API
#[derive(Serialize)]
pub struct PluginResponse {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub kind: String,
    pub enabled: bool,
    pub tool_count: usize,
}

/// Build plugin management routes
pub fn router(manager: SharedPluginManager) -> Router {
    Router::new()
        .route("/", get(list_plugins))
        .route("/{id}/enable", post(enable_plugin))
        .route("/{id}/disable", post(disable_plugin))
        .with_state(manager)
}

/// List all installed plugins
async fn list_plugins(
    State(manager): State<SharedPluginManager>,
) -> Json<Vec<PluginResponse>> {
    let mgr = manager.lock().await;
    let plugins: Vec<PluginResponse> = mgr
        .list()
        .iter()
        .map(|p| PluginResponse {
            id: p.manifest.id.clone(),
            name: p.manifest.name.clone(),
            version: p.manifest.version.clone(),
            description: p.manifest.description.clone(),
            author: p.manifest.author.clone(),
            kind: serde_json::to_value(&p.manifest.kind)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| format!("{:?}", p.manifest.kind)),
            enabled: p.enabled,
            tool_count: p.manifest.tools.len(),
        })
        .collect();
    Json(plugins)
}

/// Enable a plugin
async fn enable_plugin(
    State(manager): State<SharedPluginManager>,
    Path(id): Path<String>,
) -> StatusCode {
    let mut mgr = manager.lock().await;
    if mgr.enable(&id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Disable a plugin
async fn disable_plugin(
    State(manager): State<SharedPluginManager>,
    Path(id): Path<String>,
) -> StatusCode {
    let mut mgr = manager.lock().await;
    if mgr.disable(&id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}
