//! Admin API endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{auth::require_api_key, ApiState};
use crate::db::{SessionRepo, TelegramGroupConfig, UserRepo};

// --- Request/Response types ---

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub id: String,
    #[serde(default)]
    pub life_json_path: Option<String>,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: String,
    pub life_json_path: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct SetLifeJsonRequest {
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_id: String,
    pub channel: String,
    pub channel_id: String,
    pub persona_id: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct MessageResponse {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
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

/// Create a new user
async fn create_user(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), (StatusCode, Json<ErrorResponse>)> {
    let user_repo = UserRepo::new(state.db.clone());

    let user = user_repo
        .find_or_create(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    // Set life.json path if provided
    if let Some(path) = &req.life_json_path {
        user_repo
            .set_life_json_path(&user.id, Some(path))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;
    }

    Ok((
        StatusCode::CREATED,
        Json(UserResponse {
            id: user.id,
            life_json_path: req.life_json_path,
            created_at: user.created_at.to_rfc3339(),
        }),
    ))
}

/// List all users
async fn list_users(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<UserResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let user_repo = UserRepo::new(state.db.clone());

    let users = user_repo
        .list_all()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(Json(
        users
            .into_iter()
            .map(|u| UserResponse {
                id: u.id,
                life_json_path: u.life_json_path,
                created_at: u.created_at.to_rfc3339(),
            })
            .collect(),
    ))
}

/// Get a specific user
async fn get_user(
    State(state): State<Arc<ApiState>>,
    Path(user_id): Path<String>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    let user_repo = UserRepo::new(state.db.clone());

    let user = user_repo
        .find(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, error_response("not_found", "User not found")))?;

    Ok(Json(UserResponse {
        id: user.id,
        life_json_path: user.life_json_path,
        created_at: user.created_at.to_rfc3339(),
    }))
}

/// Set a user's life.json path
async fn set_life_json(
    State(state): State<Arc<ApiState>>,
    Path(user_id): Path<String>,
    Json(req): Json<SetLifeJsonRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let user_repo = UserRepo::new(state.db.clone());

    // Verify user exists
    user_repo
        .find(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, error_response("not_found", "User not found")))?;

    user_repo
        .set_life_json_path(&user_id, req.path.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Delete a user
async fn delete_user(
    State(state): State<Arc<ApiState>>,
    Path(user_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let user_repo = UserRepo::new(state.db.clone());

    user_repo
        .delete(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(StatusCode::NO_CONTENT)
}

/// List sessions (optionally filtered by user)
async fn list_sessions(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<SessionResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let session_repo = SessionRepo::new(state.db.clone());

    let sessions = session_repo
        .list_all()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionResponse {
                id: s.id,
                user_id: s.user_id,
                channel: s.channel,
                channel_id: s.channel_id,
                persona_id: s.persona_id,
                created_at: s.created_at.to_rfc3339(),
            })
            .collect(),
    ))
}

/// Get messages for a session
async fn get_session_messages(
    State(state): State<Arc<ApiState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<MessageResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let session_repo = SessionRepo::new(state.db.clone());

    let messages = session_repo
        .get_messages(&session_id, 100)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(Json(
        messages
            .into_iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: format!("{:?}", m.role).to_lowercase(),
                content: m.content,
                created_at: m.created_at.to_rfc3339(),
            })
            .collect(),
    ))
}

// --- Telegram group config handlers ---

/// List Telegram group configurations
async fn list_telegram_groups(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Vec<TelegramGroupConfig>>, (StatusCode, Json<ErrorResponse>)> {
    let configs = state
        .telegram_group_repo
        .list()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(Json(configs))
}

/// Upsert Telegram group configuration
async fn upsert_telegram_group(
    State(state): State<Arc<ApiState>>,
    Path(chat_id): Path<String>,
    Json(mut config): Json<TelegramGroupConfig>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    // Ensure path param matches body
    config.chat_id = chat_id;

    state
        .telegram_group_repo
        .upsert(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    Ok(StatusCode::OK)
}

/// Delete Telegram group configuration
async fn delete_telegram_group(
    State(state): State<Arc<ApiState>>,
    Path(chat_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let deleted = state
        .telegram_group_repo
        .delete(&chat_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, error_response("db_error", &e.to_string())))?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, error_response("not_found", "Group config not found")))
    }
}

/// Build admin router with auth middleware
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/users", post(create_user))
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}/life-json", put(set_life_json))
        .route("/users/{id}", delete(delete_user))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}/messages", get(get_session_messages))
        .route("/telegram/groups", get(list_telegram_groups))
        .route("/telegram/groups/{chat_id}", put(upsert_telegram_group))
        .route("/telegram/groups/{chat_id}", delete(delete_telegram_group))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
}
