//! life.json memory export/import API endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{auth::require_api_key, ApiState};
use crate::context::life_json_sync;
use crate::db::{Memory, MemoryCategory};

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

/// A single memory item in API responses
#[derive(Debug, Serialize)]
pub struct MemoryItem {
    pub id: String,
    pub category: String,
    pub content: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub access_count: u32,
    pub created_at: String,
    pub accessed_at: String,
    pub source_channel: Option<String>,
}

impl From<&crate::db::Memory> for MemoryItem {
    fn from(m: &crate::db::Memory) -> Self {
        Self {
            id: m.id.clone(),
            category: m.category.to_string(),
            content: m.content.clone(),
            tags: m.tags.clone(),
            pinned: m.pinned,
            access_count: m.access_count,
            created_at: m.created_at.to_rfc3339(),
            accessed_at: m.accessed_at.to_rfc3339(),
            source_channel: m.source_channel.clone(),
        }
    }
}

/// Query parameters for listing/searching memories
#[derive(Debug, Deserialize)]
pub struct MemoryListQuery {
    /// User ID (required)
    pub user_id: String,
    /// Optional category filter
    pub category: Option<String>,
    /// Optional keyword search query
    pub q: Option<String>,
    /// Max results (default: 50)
    pub limit: Option<usize>,
}

/// Request body for adding a memory
#[derive(Debug, Deserialize)]
pub struct AddMemoryRequest {
    pub user_id: String,
    pub content: String,
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

/// Response for list/search endpoints
#[derive(Debug, Serialize)]
pub struct MemoryListResponse {
    pub memories: Vec<MemoryItem>,
    pub count: usize,
}

/// Memory item DTO (embedding excluded for API responses)
#[derive(Serialize)]
pub struct MemoryDto {
    pub id: String,
    pub user_id: String,
    pub category: String,
    pub content: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub access_count: u32,
    pub created_at: String,
    pub accessed_at: String,
    pub source_session_id: Option<String>,
    pub source_channel: Option<String>,
}

impl From<&Memory> for MemoryDto {
    fn from(m: &Memory) -> Self {
        Self {
            id: m.id.clone(),
            user_id: m.user_id.clone(),
            category: m.category.to_string(),
            content: m.content.clone(),
            tags: m.tags.clone(),
            pinned: m.pinned,
            access_count: m.access_count,
            created_at: m.created_at.to_rfc3339(),
            accessed_at: m.accessed_at.to_rfc3339(),
            source_session_id: m.source_session_id.clone(),
            source_channel: m.source_channel.clone(),
        }
    }
}

/// Query parameters for the CRUD list endpoint
#[derive(Deserialize)]
pub struct ListQuery {
    pub user_id: String,
    pub category: Option<String>,
    pub limit: Option<usize>,
}

/// Query parameters for the memory search endpoint
#[derive(Deserialize)]
pub struct MemorySearchQuery {
    pub user_id: String,
    pub q: String,
    pub limit: Option<usize>,
}

/// Request body for creating a memory via the CRUD endpoint
#[derive(Deserialize)]
pub struct CreateMemoryRequest {
    pub user_id: String,
    pub content: String,
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response for CRUD list/search endpoints
#[derive(Serialize)]
pub struct ListResponse {
    pub memories: Vec<MemoryDto>,
    pub count: usize,
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

/// List memories for a user (CRUD endpoint)
async fn crud_list_memories(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let category = query.category.as_deref().and_then(MemoryCategory::from_str_value);
    let memories = state
        .memory_repo
        .list(&query.user_id, category)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    let mut dtos: Vec<MemoryDto> = memories.iter().map(MemoryDto::from).collect();
    if let Some(limit) = query.limit {
        dtos.truncate(limit);
    }
    let count = dtos.len();
    Ok(Json(ListResponse { memories: dtos, count }))
}

/// Search memories by text query (CRUD endpoint)
async fn search_memories(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<MemorySearchQuery>,
) -> Result<Json<ListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(10);
    let memories = state
        .memory_repo
        .search(&query.user_id, &query.q)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    let dtos: Vec<MemoryDto> = memories.iter().take(limit).map(MemoryDto::from).collect();
    let count = dtos.len();
    Ok(Json(ListResponse { memories: dtos, count }))
}

/// Create a new memory (CRUD endpoint)
async fn create_memory(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateMemoryRequest>,
) -> Result<(StatusCode, Json<MemoryDto>), (StatusCode, Json<ErrorResponse>)> {
    let category = req
        .category
        .as_deref()
        .and_then(MemoryCategory::from_str_value)
        .unwrap_or(MemoryCategory::General);

    let mut memory = Memory::new(req.user_id, category, req.content);
    for tag in req.tags {
        memory = memory.with_tag(tag);
    }

    state
        .memory_repo
        .add(&memory)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok((StatusCode::CREATED, Json(MemoryDto::from(&memory))))
}

/// Soft-delete a memory by ID
async fn delete_memory(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let deleted = state.memory_repo.delete(&id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_response("db_error", &e.to_string()),
        )
    })?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            error_response("not_found", &format!("memory {id} not found")),
        ))
    }
}

/// Build the memory API router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        // life.json sync endpoints (existing)
        .route("/export", get(export_memories))
        .route("/import", post(import_memories))
        // Memory management CRUD
        .route("/", get(crud_list_memories).post(create_memory))
        .route("/search", get(search_memories))
        .route("/{id}", delete(delete_memory))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, MemoryCategory, MemoryRepo};

    fn make_repo() -> MemoryRepo {
        let pool = db::init_memory().unwrap();
        MemoryRepo::new(pool)
    }

    #[test]
    fn list_handler_filters_by_user() {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool.clone());
        let user_repo = crate::db::UserRepo::new(pool);

        let alice = user_repo.find_or_create("alice_lhfbu").unwrap();
        let bob = user_repo.find_or_create("bob_lhfbu").unwrap();

        let m1 = crate::db::Memory::new(alice.id.clone(), MemoryCategory::Fact, "Alice fact".to_string());
        let m2 = crate::db::Memory::new(bob.id.clone(), MemoryCategory::Fact, "Bob fact".to_string());
        repo.add(&m1).unwrap();
        repo.add(&m2).unwrap();

        let alice_memories = repo.list(&alice.id, None).unwrap();
        let bob_memories = repo.list(&bob.id, None).unwrap();
        assert_eq!(alice_memories.len(), 1);
        assert_eq!(bob_memories.len(), 1);
        assert!(alice_memories[0].content.contains("Alice"));
        assert!(bob_memories[0].content.contains("Bob"));
    }

    #[test]
    fn delete_handler_soft_deletes() {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool.clone());
        let user_repo = crate::db::UserRepo::new(pool);

        let user = user_repo.find_or_create("user_dhsd_delete").unwrap();
        let m = crate::db::Memory::new(user.id.clone(), MemoryCategory::General, "to delete".to_string());
        repo.add(&m).unwrap();

        let deleted = repo.delete(&m.id).unwrap();
        assert!(deleted);

        // Should not appear in list after soft-delete
        let memories = repo.list(&user.id, None).unwrap();
        assert!(memories.is_empty());
    }

    #[test]
    fn memory_dto_omits_embedding() {
        let m = crate::db::Memory::new("u1".to_string(), MemoryCategory::Fact, "test".to_string());
        let dto = MemoryDto::from(&m);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("embedding"), "DTO must not include embedding field");
    }

    #[test]
    fn memory_item_from_memory() {
        let mem = crate::db::Memory::new(
            "u1".to_string(),
            crate::db::MemoryCategory::Preference,
            "Likes dark mode".to_string(),
        );
        let item = MemoryItem::from(&mem);
        assert_eq!(item.content, "Likes dark mode");
        assert_eq!(item.category, "preference");
        assert!(!item.pinned);
    }

    #[test]
    fn add_memory_request_deserializes() {
        let json =
            r#"{"user_id":"u1","content":"Uses vim","category":"preference","tags":["editor"]}"#;
        let req: AddMemoryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.content, "Uses vim");
        assert_eq!(req.category, Some("preference".to_string()));
        assert_eq!(req.tags, vec!["editor"]);
    }

    #[test]
    fn add_memory_request_defaults() {
        let json = r#"{"user_id":"u1","content":"Some fact"}"#;
        let req: AddMemoryRequest = serde_json::from_str(json).unwrap();
        assert!(req.category.is_none());
        assert!(req.tags.is_empty());
        assert!(!req.pinned);
    }

    #[test]
    fn memory_list_response_serializes() {
        let resp = MemoryListResponse {
            memories: vec![],
            count: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"count\":0"));
    }
}
