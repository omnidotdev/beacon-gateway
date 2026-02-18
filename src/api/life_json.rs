//! life.json memory export/import API endpoints

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{auth::require_api_key, ApiState};
use crate::context::life_json_sync;

// --- Request/Response types ---

/// Query parameters for export
#[derive(Deserialize)]
pub struct ExportQuery {
    /// User ID to export memories for
    pub user_id: String,
    /// Persona ID for the assistants key (defaults to active persona)
    pub persona_id: Option<String>,
    /// Max number of memories to export
    pub limit: Option<usize>,
}

/// Export response
#[derive(Serialize)]
pub struct ExportResponse {
    /// The life.json content with exported memories
    pub life_json: crate::context::LifeJson,
    /// Number of memories exported
    pub count: usize,
}

/// Request body for import
#[derive(Deserialize)]
pub struct ImportRequest {
    /// User ID to import memories for
    pub user_id: String,
    /// Raw life.json content as a JSON string or object
    pub content: serde_json::Value,
    /// Optional persona ID to filter imports to a single assistant section
    pub persona_id: Option<String>,
}

/// Import response
#[derive(Serialize)]
pub struct ImportResponse {
    /// Number of memories imported
    pub imported: usize,
    /// Number of memories skipped (duplicates)
    pub skipped: usize,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

fn error_response(code: &str, message: &str) -> Json<ErrorResponse> {
    Json(ErrorResponse {
        error: ErrorDetail {
            code: code.to_string(),
            message: message.to_string(),
        },
    })
}

// --- Handlers ---

/// Export memories as life.json format
async fn export_memories(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<ExportResponse>, (StatusCode, Json<ErrorResponse>)> {
    let persona_id = query
        .persona_id
        .unwrap_or_else(|| state.persona_id.clone());

    let result = life_json_sync::export_memories(
        &state.memory_repo,
        &query.user_id,
        &persona_id,
        query.limit,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_response("export_error", &e.to_string()),
        )
    })?;

    Ok(Json(ExportResponse {
        life_json: result.life_json,
        count: result.count,
    }))
}

/// Import memories from life.json content
async fn import_memories(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), (StatusCode, Json<ErrorResponse>)> {
    // Normalize content to a JSON string regardless of input format
    let content_str = match req.content {
        serde_json::Value::String(s) => s,
        other => serde_json::to_string(&other).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                error_response("invalid_content", &e.to_string()),
            )
        })?,
    };

    let result = life_json_sync::import_memories(
        &state.memory_repo,
        &req.user_id,
        &content_str,
        req.persona_id.as_deref(),
    )
    .map_err(|e| {
        let (status, code) = if e.to_string().contains("parse") || e.to_string().contains("JSON") {
            (StatusCode::BAD_REQUEST, "parse_error")
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, "import_error")
        };
        (status, error_response(code, &e.to_string()))
    })?;

    Ok((
        StatusCode::OK,
        Json(ImportResponse {
            imported: result.imported,
            skipped: result.skipped,
        }),
    ))
}

/// Build the life.json memory sync router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/export", get(export_memories))
        .route("/import", post(import_memories))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .with_state(state)
}
