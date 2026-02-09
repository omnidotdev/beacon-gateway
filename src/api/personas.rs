//! Personas API endpoints for marketplace integration

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::ApiState;
use crate::db::PersonaRepo;
use crate::skills::ManifoldClient;
use crate::Persona;

/// Build personas router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(list_installed))
        .route("/search", get(search))
        .route("/install", post(install))
        .route("/{persona_id}", get(get_persona).delete(uninstall))
        .with_state(state)
}

/// Persona info for API responses
#[derive(Debug, Serialize)]
pub struct PersonaResponse {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub source: PersonaSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersonaSource {
    Local,
    Manifold { namespace: String },
}

impl From<&Persona> for PersonaResponse {
    fn from(p: &Persona) -> Self {
        let avatar = p
            .branding
            .as_ref()
            .and_then(|b| b.assets.as_ref())
            .and_then(|a| a.avatar.clone());

        let accent_color = p
            .branding
            .as_ref()
            .and_then(|b| b.colors.as_ref())
            .and_then(|c| c.primary.clone());

        Self {
            id: p.identity.id.clone(),
            name: p.identity.name.clone(),
            tagline: p.identity.tagline.clone(),
            avatar,
            accent_color,
            icon: p.identity.icon.clone(),
            source: PersonaSource::Local,
            installed_at: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PersonaListResponse {
    pub personas: Vec<PersonaResponse>,
    pub total: usize,
}

/// Search query params
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    pub namespace: Option<String>,
}

/// Install request
#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub namespace: String,
    pub persona_id: String,
}

/// List installed personas (from marketplace)
async fn list_installed(State(state): State<Arc<ApiState>>) -> Json<PersonaListResponse> {
    let persona_repo = PersonaRepo::new(state.db.clone());

    let installed = persona_repo.list().unwrap_or_default();

    let personas: Vec<PersonaResponse> = installed
        .into_iter()
        .map(|ip| {
            let mut resp = PersonaResponse::from(&ip.persona);
            resp.source = PersonaSource::Manifold {
                namespace: ip.source_namespace,
            };
            resp.installed_at = Some(ip.installed_at.to_rfc3339());
            resp
        })
        .collect();

    let total = personas.len();

    Json(PersonaListResponse { personas, total })
}

/// Search personas in marketplace
async fn search(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<PersonaListResponse>, (StatusCode, Json<ApiError>)> {
    let client = ManifoldClient::new(&state.manifold_url);

    let personas = client.search_personas(&query.q).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                code: "manifold_error".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    let responses: Vec<PersonaResponse> = personas
        .iter()
        .map(|p| {
            let mut resp = PersonaResponse::from(p);
            resp.source = PersonaSource::Manifold {
                namespace: query.namespace.clone().unwrap_or_else(|| "community".to_string()),
            };
            resp
        })
        .collect();

    let total = responses.len();

    Ok(Json(PersonaListResponse {
        personas: responses,
        total,
    }))
}

/// Get a specific installed persona
async fn get_persona(
    State(state): State<Arc<ApiState>>,
    Path(persona_id): Path<String>,
) -> Result<Json<PersonaResponse>, (StatusCode, Json<ApiError>)> {
    let persona_repo = PersonaRepo::new(state.db.clone());

    let installed = persona_repo.get(&persona_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                code: "database_error".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    match installed {
        Some(ip) => {
            let mut resp = PersonaResponse::from(&ip.persona);
            resp.source = PersonaSource::Manifold {
                namespace: ip.source_namespace,
            };
            resp.installed_at = Some(ip.installed_at.to_rfc3339());
            Ok(Json(resp))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                code: "not_found".to_string(),
                message: format!("persona not found: {persona_id}"),
            }),
        )),
    }
}

/// Install a persona from marketplace
async fn install(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<PersonaResponse>, (StatusCode, Json<ApiError>)> {
    let client = ManifoldClient::new(&state.manifold_url);

    // Fetch persona from Manifold
    let persona = client
        .get_persona(&req.namespace, &req.persona_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiError {
                    code: "manifold_error".to_string(),
                    message: e.to_string(),
                }),
            )
        })?;

    // Store in database
    let persona_repo = PersonaRepo::new(state.db.clone());
    let installed = persona_repo.install(&persona, &req.namespace).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                code: "database_error".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    let mut resp = PersonaResponse::from(&installed.persona);
    resp.source = PersonaSource::Manifold {
        namespace: installed.source_namespace,
    };
    resp.installed_at = Some(installed.installed_at.to_rfc3339());

    Ok(Json(resp))
}

/// Uninstall a persona
async fn uninstall(
    State(state): State<Arc<ApiState>>,
    Path(persona_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let persona_repo = PersonaRepo::new(state.db.clone());

    let removed = persona_repo.uninstall(&persona_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                code: "database_error".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                code: "not_found".to_string(),
                message: format!("persona not found: {persona_id}"),
            }),
        ))
    }
}

#[derive(Debug, Serialize)]
struct ApiError {
    code: String,
    message: String,
}
