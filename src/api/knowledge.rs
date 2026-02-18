//! Knowledge pack API endpoints

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
use crate::knowledge::{select_knowledge, KnowledgePackResolver};
use crate::persona::{KnowledgeChunk, KnowledgePackRef};
use crate::skills::ManifoldClient;

// --- Request/Response types ---

/// Summary of a knowledge pack
#[derive(Serialize)]
pub struct PackSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub chunks: usize,
    pub tags: Vec<String>,
    pub version: String,
}

/// Response for listing cached packs
#[derive(Serialize)]
pub struct PackListResponse {
    pub packs: Vec<PackSummary>,
    pub total: usize,
}

/// Response for searching packs on Manifold
#[derive(Serialize)]
pub struct PackSearchResponse {
    pub packs: Vec<PackSummary>,
    pub total: usize,
}

/// Request body to install a pack by ref
#[derive(Deserialize)]
pub struct InstallPackRequest {
    /// Manifold artifact ref, e.g. `@community/knowledge/solana-defi`
    #[serde(rename = "ref")]
    pub pack_ref: String,
    /// Optional semver version constraint
    pub version: Option<String>,
}

/// Query parameters for search
#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

/// Query parameters for chunk preview
#[derive(Deserialize)]
pub struct ChunkPreviewQuery {
    pub message: String,
    /// Optional max token budget (defaults to state `max_context_tokens`)
    pub max_tokens: Option<usize>,
}

/// A single knowledge chunk in API responses
#[derive(Serialize)]
pub struct ChunkResponse {
    pub topic: String,
    pub tags: Vec<String>,
    pub content: String,
    pub rules: Vec<String>,
    pub priority: String,
}

/// Response for chunk preview
#[derive(Serialize)]
pub struct ChunkPreviewResponse {
    pub chunks: Vec<ChunkResponse>,
    pub total: usize,
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

fn chunk_to_response(chunk: &KnowledgeChunk) -> ChunkResponse {
    ChunkResponse {
        topic: chunk.topic.clone(),
        tags: chunk.tags.clone(),
        content: chunk.content.clone(),
        rules: chunk.rules.clone(),
        priority: format!("{:?}", chunk.priority).to_lowercase(),
    }
}

// --- Handlers ---

/// List cached knowledge packs from the local cache directory
async fn list_packs(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<PackListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let packs = read_cached_packs(&state.knowledge_cache_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_response("cache_error", &e.to_string()),
        )
    })?;

    let total = packs.len();

    Ok(Json(PackListResponse { packs, total }))
}

/// Search Manifold for knowledge packs
async fn search_packs(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<PackSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = ManifoldClient::new(&state.manifold_url);

    let q = query.q.as_deref().unwrap_or("");
    let results = client.search_knowledge_packs(q).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            error_response("manifold_error", &e.to_string()),
        )
    })?;

    let packs: Vec<PackSummary> = results
        .iter()
        .map(|p| PackSummary {
            name: p.name.clone(),
            description: p.description.clone(),
            chunks: p.chunks.len(),
            tags: p.tags.clone(),
            version: p.version.clone(),
        })
        .collect();

    let total = packs.len();

    Ok(Json(PackSearchResponse { packs, total }))
}

/// Install a knowledge pack from Manifold by ref
async fn install_pack(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<InstallPackRequest>,
) -> Result<(StatusCode, Json<PackSummary>), (StatusCode, Json<ErrorResponse>)> {
    let pack_ref = KnowledgePackRef {
        pack_ref: req.pack_ref.clone(),
        version: req.version,
        priority: None,
    };

    let resolver =
        KnowledgePackResolver::new(&state.manifold_url, state.knowledge_cache_dir.clone());

    let pack = resolver.resolve(&pack_ref).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            error_response("resolve_error", &e.to_string()),
        )
    })?;

    tracing::info!(
        name = %pack.name,
        chunks = pack.chunks.len(),
        "installed knowledge pack via API"
    );

    let summary = PackSummary {
        name: pack.name,
        description: pack.description,
        chunks: pack.chunks.len(),
        tags: pack.tags,
        version: pack.version,
    };

    Ok((StatusCode::CREATED, Json(summary)))
}

/// Remove a cached knowledge pack by name
async fn remove_pack(
    State(state): State<Arc<ApiState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let removed = remove_cached_pack(&state.knowledge_cache_dir, &name).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_response("cache_error", &e.to_string()),
        )
    })?;

    if removed {
        tracing::info!(name = %name, "removed cached knowledge pack");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            error_response("not_found", "Knowledge pack not found in cache"),
        ))
    }
}

/// Preview which chunks would be selected for a given message
async fn preview_chunks(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ChunkPreviewQuery>,
) -> Json<ChunkPreviewResponse> {
    let max_tokens = query.max_tokens.unwrap_or(state.max_context_tokens);
    let selected = select_knowledge(&state.persona_knowledge, &query.message, max_tokens);

    let chunks: Vec<ChunkResponse> = selected.iter().map(|c| chunk_to_response(c)).collect();
    let total = chunks.len();

    Json(ChunkPreviewResponse { chunks, total })
}

// --- Cache helpers ---

/// Read all cached knowledge packs from the cache directory
fn read_cached_packs(
    cache_dir: &std::path::Path,
) -> std::result::Result<Vec<PackSummary>, std::io::Error> {
    let mut packs = Vec::new();

    if !cache_dir.exists() {
        return Ok(packs);
    }

    // Cache layout: {cache_dir}/{namespace}/{pack_name}/{version}.json
    for ns_entry in std::fs::read_dir(cache_dir)?.flatten() {
        let ns_path = ns_entry.path();
        if !ns_path.is_dir() {
            continue;
        }

        for pack_entry in std::fs::read_dir(&ns_path)?.flatten() {
            let pack_path = pack_entry.path();
            if !pack_path.is_dir() {
                continue;
            }

            for file_entry in std::fs::read_dir(&pack_path)?.flatten() {
                let file_path = file_entry.path();
                if file_path.extension().is_some_and(|ext| ext == "json") {
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        if let Ok(pack) =
                            serde_json::from_str::<crate::persona::KnowledgePack>(&content)
                        {
                            packs.push(PackSummary {
                                name: pack.name,
                                description: pack.description,
                                chunks: pack.chunks.len(),
                                tags: pack.tags,
                                version: pack.version,
                            });
                        }
                    }
                }
            }
        }
    }

    packs.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(packs)
}

/// Remove a cached knowledge pack by name
///
/// Searches all namespace/pack directories for matching pack files
fn remove_cached_pack(
    cache_dir: &std::path::Path,
    name: &str,
) -> std::result::Result<bool, std::io::Error> {
    if !cache_dir.exists() {
        return Ok(false);
    }

    let mut removed = false;

    for ns_entry in std::fs::read_dir(cache_dir)?.flatten() {
        let ns_path = ns_entry.path();
        if !ns_path.is_dir() {
            continue;
        }

        for pack_entry in std::fs::read_dir(&ns_path)?.flatten() {
            let pack_path = pack_entry.path();
            if !pack_path.is_dir() {
                continue;
            }

            for file_entry in std::fs::read_dir(&pack_path)?.flatten() {
                let file_path = file_entry.path();
                if file_path.extension().is_some_and(|ext| ext == "json") {
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        if let Ok(pack) =
                            serde_json::from_str::<crate::persona::KnowledgePack>(&content)
                        {
                            if pack.name == name {
                                std::fs::remove_file(&file_path)?;
                                removed = true;

                                // Clean up empty parent directories
                                if pack_path
                                    .read_dir()
                                    .map_or(false, |mut d| d.next().is_none())
                                {
                                    let _ = std::fs::remove_dir(&pack_path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(removed)
}

/// Build the knowledge router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/packs", get(list_packs))
        .route("/search", get(search_packs))
        .route("/install", post(install_pack))
        .route("/packs/{name}", delete(remove_pack))
        .route("/chunks", get(preview_chunks))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .with_state(state)
}
