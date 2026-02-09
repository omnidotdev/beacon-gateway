//! Health check endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use super::ApiState;
use crate::{Config, Persona};

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

/// Detailed readiness response
#[derive(Serialize)]
pub struct ReadinessResponse {
    pub status: &'static str,
    pub checks: ReadinessChecks,
}

/// Individual readiness checks
#[derive(Serialize)]
pub struct ReadinessChecks {
    pub database: CheckResult,
    pub agent: CheckResult,
}

/// Result of a single health check
#[derive(Serialize)]
pub struct CheckResult {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl CheckResult {
    const fn ok() -> Self {
        Self {
            status: "ok",
            message: None,
        }
    }

    fn fail(message: impl Into<String>) -> Self {
        Self {
            status: "fail",
            message: Some(message.into()),
        }
    }

    fn unavailable() -> Self {
        Self {
            status: "unavailable",
            message: Some("not configured".to_string()),
        }
    }
}

/// Liveness probe - is the service running?
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Readiness probe - is the service ready to accept traffic?
async fn ready(State(state): State<Arc<ApiState>>) -> (StatusCode, Json<ReadinessResponse>) {
    let db_check = check_database(&state);
    let agent_check = check_agent(&state);

    let all_ok = db_check.status == "ok"
        && (agent_check.status == "ok" || agent_check.status == "unavailable");

    let status = if all_ok { "ok" } else { "degraded" };
    let http_status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        http_status,
        Json(ReadinessResponse {
            status,
            checks: ReadinessChecks {
                database: db_check,
                agent: agent_check,
            },
        }),
    )
}

/// Check database connectivity
fn check_database(state: &ApiState) -> CheckResult {
    match state.db.get() {
        Ok(conn) => {
            // Try a simple query to verify the connection works
            match conn.query_row("SELECT 1", [], |_| Ok(())) {
                Ok(()) => CheckResult::ok(),
                Err(e) => CheckResult::fail(format!("query failed: {e}")),
            }
        }
        Err(e) => CheckResult::fail(format!("connection failed: {e}")),
    }
}

/// Check agent availability
fn check_agent(state: &ApiState) -> CheckResult {
    match &state.agent {
        Some(_) => CheckResult::ok(),
        None => CheckResult::unavailable(),
    }
}

/// Build health router (liveness only, no state needed)
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

/// System status response including model info
#[derive(Serialize)]
pub struct StatusResponse {
    pub version: &'static str,
    pub persona_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelStatus>,
    pub voice_available: bool,
}

#[derive(Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub provider: String,
}

/// Persona info for API responses
#[derive(Serialize)]
pub struct PersonaInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
}

/// Response for listing all personas
#[derive(Serialize)]
pub struct PersonaListResponse {
    pub personas: Vec<PersonaInfo>,
    pub active_id: String,
}

/// Get system status including current model
async fn status(State(state): State<Arc<ApiState>>) -> Json<StatusResponse> {
    let model = state.model_info.as_ref().map(|m| ModelStatus {
        id: m.model_id.clone(),
        provider: m.provider.clone(),
    });

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        persona_id: state.persona_id.clone(),
        model,
        voice_available: state.stt.is_some() && state.tts.is_some(),
    })
}

/// Get current persona info
async fn get_persona(State(state): State<Arc<ApiState>>) -> Json<PersonaInfo> {
    let persona = load_persona_file(&state.persona_cache_dir, &state.persona_id)
        .or_else(|| {
            Config::load_embedded_persona(&state.persona_id)
                .ok()
                .map(|p| persona_to_info(&p))
        });
    Json(persona.unwrap_or_else(|| PersonaInfo {
        id: state.persona_id.clone(),
        name: capitalize_first(&state.persona_id),
        tagline: Some("AI Assistant".to_string()),
        avatar: None,
        accent_color: Some("#4ecdc4".to_string()),
    }))
}

/// List all available personas
async fn list_personas(State(state): State<Arc<ApiState>>) -> Json<PersonaListResponse> {
    let mut personas = load_all_personas(&state.persona_cache_dir);

    // Merge in embedded personas not already in cache
    let cached_ids: std::collections::HashSet<String> =
        personas.iter().map(|p| p.id.clone()).collect();
    for (_, json) in Config::embedded_personas() {
        if let Ok(persona) = serde_json::from_str::<Persona>(json) {
            if !cached_ids.contains(&persona.identity.id) {
                personas.push(persona_to_info(&persona));
            }
        }
    }

    personas.sort_by(|a, b| a.name.cmp(&b.name));

    Json(PersonaListResponse {
        personas,
        active_id: state.persona_id.clone(),
    })
}

/// Activate a persona (switch to it)
/// Updates the active persona and system prompt used for chat
async fn activate_persona(
    State(state): State<Arc<ApiState>>,
    Path(persona_id): Path<String>,
) -> Result<Json<PersonaInfo>, StatusCode> {
    let persona = load_full_persona(&state.persona_cache_dir, &persona_id)
        .or_else(|| {
            Config::load_embedded_persona(&persona_id).ok().map(|p| {
                let system_prompt = p.system_prompt().map(String::from);
                (persona_to_info(&p), system_prompt)
            })
        });

    match persona {
        None => {
            tracing::warn!(persona_id = %persona_id, "persona not found");
            Err(StatusCode::NOT_FOUND)
        }
        Some((info, system_prompt)) => {
            // Update the active persona
            {
                let mut active = state.active_persona.write().await;
                active.id = persona_id.clone();
                active.system_prompt = system_prompt;
            }
            tracing::info!(persona_id = %persona_id, "persona activated");
            Ok(Json(info))
        }
    }
}

/// Load a single persona from file
fn load_persona_file(personas_dir: &std::path::Path, persona_id: &str) -> Option<PersonaInfo> {
    load_full_persona(personas_dir, persona_id).map(|(info, _)| info)
}

/// Load a persona with its system prompt
fn load_full_persona(
    personas_dir: &std::path::Path,
    persona_id: &str,
) -> Option<(PersonaInfo, Option<String>)> {
    let json_path = personas_dir.join(format!("{persona_id}.json"));
    if !json_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&json_path).ok()?;
    let persona: Persona = serde_json::from_str(&content).ok()?;

    let system_prompt = persona.system_prompt().map(String::from);
    Some((persona_to_info(&persona), system_prompt))
}

/// Load all personas from the directory
fn load_all_personas(personas_dir: &std::path::Path) -> Vec<PersonaInfo> {
    let mut personas = Vec::new();

    let Ok(entries) = std::fs::read_dir(personas_dir) else {
        return personas;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(persona) = serde_json::from_str::<Persona>(&content) {
                    personas.push(persona_to_info(&persona));
                }
            }
        }
    }

    // Sort by name for consistent ordering
    personas.sort_by(|a, b| a.name.cmp(&b.name));

    personas
}

/// Convert a Persona to `PersonaInfo` for the API
fn persona_to_info(persona: &Persona) -> PersonaInfo {
    let avatar = persona
        .branding
        .as_ref()
        .and_then(|b| b.assets.as_ref())
        .and_then(|a| a.avatar.clone());

    let accent_color = persona
        .branding
        .as_ref()
        .and_then(|b| b.colors.as_ref())
        .and_then(|c| c.primary.clone());

    PersonaInfo {
        id: persona.identity.id.clone(),
        name: persona.identity.name.clone(),
        tagline: persona.identity.tagline.clone(),
        avatar,
        accent_color,
    }
}

/// Helper to capitalize first letter
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Gateway discovery info for pairing
#[derive(Serialize)]
pub struct GatewayInfo {
    pub device_id: String,
    pub name: String,
    pub version: &'static str,
    pub persona: String,
    pub voice: bool,
}

/// Get gateway info for discovery/pairing
async fn get_gateway_info(State(state): State<Arc<ApiState>>) -> Json<GatewayInfo> {
    Json(GatewayInfo {
        device_id: format!("beacon-{}", &state.persona_id),
        name: "Beacon Gateway".to_string(),
        version: env!("CARGO_PKG_VERSION"),
        persona: state.persona_id.clone(),
        voice: state.stt.is_some() && state.tts.is_some(),
    })
}

/// Build readiness router (needs state for checks)
pub fn ready_router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/ready", get(ready))
        .route("/api/status", get(status))
        .route("/api/persona", get(get_persona))
        .route("/api/personas", get(list_personas))
        .route("/api/personas/{persona_id}/activate", post(activate_persona))
        .route("/api/pair/gateway", get(get_gateway_info))
        .with_state(state)
}
