//! Skills API endpoints

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{auth::require_api_key, ApiState};
use crate::skills::{ManifoldClient, Skill, SkillPriority, SkillSource};

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
    pub priority: String,
    pub installed_at: Option<String>,
    pub always: bool,
    pub user_invocable: bool,
    pub command_name: Option<String>,
    pub emoji: Option<String>,
}

#[derive(Serialize)]
pub struct SkillCommandResponse {
    pub command: String,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
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
pub struct InstallLocalRequest {
    pub name: String,
    pub description: String,
    pub content: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub priority: Option<SkillPriority>,
    #[serde(default)]
    pub always: bool,
    #[serde(default = "default_user_invocable")]
    pub user_invocable: bool,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub requires_env: Vec<String>,
}

fn default_user_invocable() -> bool {
    true
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

#[derive(Deserialize)]
pub struct SetPriorityRequest {
    pub priority: SkillPriority,
}

/// Unified update request for skill configuration
#[derive(Deserialize)]
pub struct UpdateSkillRequest {
    pub enabled: Option<bool>,
    pub priority: Option<SkillPriority>,
    pub api_key: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

/// Skills status report
#[derive(Serialize)]
pub struct SkillStatusReport {
    pub managed_dir: String,
    pub personal_dir: String,
    pub skills: Vec<SkillStatusEntry>,
}

/// Individual skill status with eligibility info
#[derive(Serialize)]
pub struct SkillStatusEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceResponse,
    pub emoji: Option<String>,
    pub enabled: bool,
    pub eligible: bool,
    pub always: bool,
    pub requirements: SkillRequirements,
    pub missing: SkillRequirements,
}

/// Skill requirements lists
#[derive(Serialize)]
pub struct SkillRequirements {
    pub env: Vec<String>,
    pub bins: Vec<String>,
    pub any_bins: Vec<String>,
    pub os: Vec<String>,
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
        priority: skill.priority.as_db().to_string(),
        installed_at: Some(skill.installed_at.to_rfc3339()),
        always: skill.skill.metadata.always,
        user_invocable: skill.skill.metadata.user_invocable,
        command_name: skill.command_name.clone(),
        emoji: skill.skill.metadata.emoji.clone(),
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
        priority: SkillPriority::default().as_db().to_string(),
        installed_at: None,
        always: skill.metadata.always,
        user_invocable: skill.metadata.user_invocable,
        command_name: None,
        emoji: skill.metadata.emoji.clone(),
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

/// Install a local skill directly (no Manifold fetch)
async fn install_local(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<InstallLocalRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), (StatusCode, Json<ErrorResponse>)> {
    if let Ok(Some(_)) = state.skill_repo.get_by_name(&req.name) {
        return Err((
            StatusCode::CONFLICT,
            error_response("already_installed", "Skill is already installed"),
        ));
    }

    let skill = Skill {
        id: String::new(),
        metadata: crate::skills::SkillMetadata {
            name: req.name,
            description: req.description,
            version: req.version,
            author: req.author,
            tags: req.tags,
            permissions: vec![],
            always: req.always,
            user_invocable: req.user_invocable,
            disable_model_invocation: req.disable_model_invocation,
            emoji: req.emoji,
            requires_env: req.requires_env,
            os: vec![],
            requires_bins: vec![],
            requires_any_bins: vec![],
            primary_env: None,
            command_dispatch: None,
            command_tool: None,
        },
        content: req.content,
        source: SkillSource::Local,
    };

    let priority = req.priority.unwrap_or_default();
    let installed = state
        .skill_repo
        .install_with_priority(&skill, priority, None)
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

/// Set the priority of a skill
async fn set_priority(
    State(state): State<Arc<ApiState>>,
    Path(skill_id): Path<String>,
    Json(req): Json<SetPriorityRequest>,
) -> Result<Json<SkillResponse>, (StatusCode, Json<ErrorResponse>)> {
    let updated = state
        .skill_repo
        .set_priority(&skill_id, req.priority)
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

/// Unified update for skill configuration (enabled, priority, api_key, env)
async fn update_skill(
    State(state): State<Arc<ApiState>>,
    Path(skill_id): Path<String>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<SkillResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Apply enabled change
    if let Some(enabled) = req.enabled {
        state
            .skill_repo
            .set_enabled(&skill_id, enabled)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;
    }

    // Apply priority change
    if let Some(priority) = req.priority {
        state
            .skill_repo
            .set_priority(&skill_id, priority)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;
    }

    // Apply api_key and/or env changes
    if req.api_key.is_some() || req.env.is_some() {
        state
            .skill_repo
            .update_skill_config(
                &skill_id,
                req.api_key.as_deref(),
                req.env.as_ref(),
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;
    }

    let skill = state
        .skill_repo
        .get(&skill_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, error_response("not_found", "Skill not found")))?;

    Ok(Json(skill_to_response(&skill)))
}

/// Get full eligibility status for all skills
async fn skill_status(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<SkillStatusReport>, (StatusCode, Json<ErrorResponse>)> {
    let skills = state
        .skill_repo
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    let entries: Vec<SkillStatusEntry> = skills
        .iter()
        .map(|s| {
            let env_ok = crate::prompt::check_env_requirements_with_config(
                &s.skill.metadata.requires_env,
                s.skill.metadata.primary_env.as_deref(),
                s.api_key.as_deref(),
            );
            let os_ok = crate::prompt::check_os_requirement(&s.skill.metadata.os);
            let bins_ok = crate::prompt::check_bins_requirement(&s.skill.metadata.requires_bins);
            let any_bins_ok = crate::prompt::check_any_bins_requirement(&s.skill.metadata.requires_any_bins);

            let eligible = s.enabled && env_ok && os_ok && bins_ok && any_bins_ok;

            // Compute missing requirements
            let missing_env: Vec<String> = s.skill.metadata.requires_env
                .iter()
                .filter(|var| {
                    std::env::var(var).is_err()
                        && !(Some(var.as_str()) == s.skill.metadata.primary_env.as_deref()
                            && s.api_key.as_ref().is_some_and(|k| !k.is_empty()))
                })
                .cloned()
                .collect();

            let missing_bins: Vec<String> = s.skill.metadata.requires_bins
                .iter()
                .filter(|b| !crate::skills::has_binary(b))
                .cloned()
                .collect();

            let missing_any_bins: Vec<String> = if any_bins_ok {
                vec![]
            } else {
                s.skill.metadata.requires_any_bins.clone()
            };

            let missing_os: Vec<String> = if os_ok {
                vec![]
            } else {
                s.skill.metadata.os.clone()
            };

            SkillStatusEntry {
                id: s.skill.id.clone(),
                name: s.skill.metadata.name.clone(),
                description: s.skill.metadata.description.clone(),
                source: SkillSourceResponse::from(&s.skill.source),
                emoji: s.skill.metadata.emoji.clone(),
                enabled: s.enabled,
                eligible,
                always: s.skill.metadata.always,
                requirements: SkillRequirements {
                    env: s.skill.metadata.requires_env.clone(),
                    bins: s.skill.metadata.requires_bins.clone(),
                    any_bins: s.skill.metadata.requires_any_bins.clone(),
                    os: s.skill.metadata.os.clone(),
                },
                missing: SkillRequirements {
                    env: missing_env,
                    bins: missing_bins,
                    any_bins: missing_any_bins,
                    os: missing_os,
                },
            }
        })
        .collect();

    Ok(Json(SkillStatusReport {
        managed_dir: state.skills_config.managed_dir.to_string_lossy().to_string(),
        personal_dir: state.skills_config.personal_dir.to_string_lossy().to_string(),
        skills: entries,
    }))
}

/// List available slash commands
async fn list_commands(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<SkillCommandResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let skills = state
        .skill_repo
        .list_enabled()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    let commands: Vec<SkillCommandResponse> = skills
        .iter()
        .filter(|s| s.skill.metadata.user_invocable && s.command_name.is_some())
        .map(|s| SkillCommandResponse {
            command: s.command_name.clone().unwrap_or_default(),
            name: s.skill.metadata.name.clone(),
            description: s.skill.metadata.description.clone(),
            emoji: s.skill.metadata.emoji.clone(),
        })
        .collect();

    Ok(Json(commands))
}

/// Build the skills router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(list_installed))
        .route("/commands", get(list_commands))
        .route("/status", get(skill_status))
        .route("/search", get(search_skills))
        .route("/install", post(install_skill))
        .route("/install/local", post(install_local))
        .route("/{skill_id}", get(get_skill).patch(update_skill).delete(uninstall_skill))
        .route("/{skill_id}/enabled", patch(set_enabled))
        .route("/{skill_id}/priority", patch(set_priority))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
}
