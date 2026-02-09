//! Skills API endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{auth::require_api_key, ApiState};
use crate::skills::{ManifoldClient, Skill, SkillSource};

// --- Request/Response types ---

#[derive(Serialize)]
pub struct SkillResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub permissions: Vec<String>,
    pub source: SkillSourceResponse,
    pub enabled: bool,
    pub installed_at: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSourceResponse {
    Local,
    Manifold { namespace: String, repository: String },
    Bundled,
}

impl From<&SkillSource> for SkillSourceResponse {
    fn from(source: &SkillSource) -> Self {
        match source {
            SkillSource::Local => Self::Local,
            SkillSource::Manifold {
                namespace,
                repository,
            } => Self::Manifold {
                namespace: namespace.clone(),
                repository: repository.clone(),
            },
            SkillSource::Bundled => Self::Bundled,
        }
    }
}

#[derive(Deserialize)]
pub struct InstallSkillRequest {
    pub namespace: String,
    pub skill_id: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

fn default_namespace() -> String {
    "community".to_string()
}

#[derive(Deserialize)]
pub struct SetEnabledRequest {
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillResponse>,
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

fn skill_to_response(skill: &crate::skills::InstalledSkill) -> SkillResponse {
    SkillResponse {
        id: skill.skill.id.clone(),
        name: skill.skill.metadata.name.clone(),
        description: skill.skill.metadata.description.clone(),
        version: skill.skill.metadata.version.clone(),
        author: skill.skill.metadata.author.clone(),
        tags: skill.skill.metadata.tags.clone(),
        permissions: skill.skill.metadata.permissions.clone(),
        source: SkillSourceResponse::from(&skill.skill.source),
        enabled: skill.enabled,
        installed_at: Some(skill.installed_at.to_rfc3339()),
    }
}

fn available_skill_to_response(skill: &Skill) -> SkillResponse {
    SkillResponse {
        id: skill.id.clone(),
        name: skill.metadata.name.clone(),
        description: skill.metadata.description.clone(),
        version: skill.metadata.version.clone(),
        author: skill.metadata.author.clone(),
        tags: skill.metadata.tags.clone(),
        permissions: skill.metadata.permissions.clone(),
        source: SkillSourceResponse::from(&skill.source),
        enabled: false,
        installed_at: None,
    }
}

// --- Handlers ---

/// List installed skills
async fn list_installed(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<SkillListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let skills = state
        .skill_repo
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    let response: Vec<SkillResponse> = skills.iter().map(skill_to_response).collect();
    let total = response.len();

    Ok(Json(SkillListResponse {
        skills: response,
        total,
    }))
}

/// Get an installed skill by ID
async fn get_skill(
    State(state): State<Arc<ApiState>>,
    Path(skill_id): Path<String>,
) -> Result<Json<SkillResponse>, (StatusCode, Json<ErrorResponse>)> {
    let skill = state
        .skill_repo
        .get(&skill_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, error_response("not_found", "Skill not found")))?;

    Ok(Json(skill_to_response(&skill)))
}

/// Search available skills from Manifold
async fn search_skills(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SkillListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = ManifoldClient::new(&state.manifold_url);

    let skills = if let Some(q) = &query.q {
        client
            .search_skills(q)
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, error_response("manifold_error", &e.to_string())))?
    } else {
        client
            .list_skills(&query.namespace)
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, error_response("manifold_error", &e.to_string())))?
    };

    let response: Vec<SkillResponse> = skills.iter().map(available_skill_to_response).collect();
    let total = response.len();

    Ok(Json(SkillListResponse {
        skills: response,
        total,
    }))
}

/// Install a skill from Manifold
async fn install_skill(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<InstallSkillRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), (StatusCode, Json<ErrorResponse>)> {
    // Check if already installed
    if let Ok(Some(_)) = state.skill_repo.get_by_name(&req.skill_id) {
        return Err((
            StatusCode::CONFLICT,
            error_response("already_installed", "Skill is already installed"),
        ));
    }

    // Fetch from Manifold
    let client = ManifoldClient::new(&state.manifold_url);
    let skill = client
        .get_skill(&req.namespace, &req.skill_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, error_response("manifold_error", &e.to_string())))?;

    // Install to database
    let installed = state
        .skill_repo
        .install(&skill)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok((StatusCode::CREATED, Json(skill_to_response(&installed))))
}

/// Uninstall a skill
async fn uninstall_skill(
    State(state): State<Arc<ApiState>>,
    Path(skill_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let removed = state
        .skill_repo
        .uninstall(&skill_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, error_response("not_found", "Skill not found")))
    }
}

/// Enable or disable a skill
async fn set_enabled(
    State(state): State<Arc<ApiState>>,
    Path(skill_id): Path<String>,
    Json(req): Json<SetEnabledRequest>,
) -> Result<Json<SkillResponse>, (StatusCode, Json<ErrorResponse>)> {
    let updated = state
        .skill_repo
        .set_enabled(&skill_id, req.enabled)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, error_response("not_found", "Skill not found")));
    }

    let skill = state
        .skill_repo
        .get(&skill_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, error_response("not_found", "Skill not found")))?;

    Ok(Json(skill_to_response(&skill)))
}

/// Build the skills router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(list_installed))
        .route("/search", get(search_skills))
        .route("/install", post(install_skill))
        .route("/{skill_id}", get(get_skill))
        .route("/{skill_id}", delete(uninstall_skill))
        .route("/{skill_id}/enabled", patch(set_enabled))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
}
