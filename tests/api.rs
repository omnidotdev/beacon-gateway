//! API endpoint integration tests

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use beacon_gateway::{Canvas, DbPool, ToolPolicy, ToolPolicyConfig};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

mod common;
use common::{create_test_session, create_test_user, setup_test_db};

/// Build a test API router
fn build_test_router(db: DbPool) -> axum::Router {
    use axum::Router;
    use beacon_gateway::db::{MemoryRepo, SessionRepo, SkillRepo, UserRepo};

    let tool_policy = Arc::new(ToolPolicy::new(&ToolPolicyConfig::default()));
    let session_repo = SessionRepo::new(db.clone());
    let user_repo = UserRepo::new(db.clone());
    let memory_repo = MemoryRepo::new(db.clone());
    let skill_repo = SkillRepo::new(db.clone());

    let canvas = Arc::new(Mutex::new(Canvas::new()));

    let state = Arc::new(beacon_gateway::api::ApiState {
        db,
        api_key: Some("test-api-key".to_string()),
        persona_id: "test-persona".to_string(),
        persona_system_prompt: None,
        persona_cache_dir: std::path::PathBuf::from("/tmp/test-persona-cache"),
        synapse: None,
        llm_model: "test-model".to_string(),
        llm_max_tokens: 1024,
        system_prompt: String::new(),
        telegram: None,
        teams: None,
        session_repo,
        user_repo,
        memory_repo,
        skill_repo,
        tool_policy,
        manifold_url: "https://manifold.omni.dev".to_string(),
        stt_model: "whisper-1".to_string(),
        tts_model: "tts-1".to_string(),
        tts_voice: "alloy".to_string(),
        tts_speed: 1.0,
        model_info: None,
        canvas,
        key_resolver: None,
        jwt_cache: None,
        persona_knowledge: vec![],
        max_context_tokens: 8000,
        active_persona: Arc::new(RwLock::new(beacon_gateway::api::ActivePersona {
            id: "test-persona".to_string(),
            system_prompt: None,
        })),
    });

    Router::new()
        .nest(
            "/api/admin",
            beacon_gateway::api::admin::router(state.clone()),
        )
        .merge(beacon_gateway::api::health::router())
        .merge(beacon_gateway::api::health::ready_router(state))
}

#[tokio::test]
async fn test_health_endpoint() {
    let db = setup_test_db();
    let app = build_test_router(db);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn test_ready_endpoint() {
    let db = setup_test_db();
    let app = build_test_router(db);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Should have detailed checks
    assert_eq!(json["status"], "ok");
    assert_eq!(json["checks"]["database"]["status"], "ok");
    assert_eq!(json["checks"]["agent"]["status"], "unavailable"); // No agent configured in tests
}

#[tokio::test]
async fn test_admin_sessions_requires_auth() {
    let db = setup_test_db();
    let app = build_test_router(db);

    // Request without API key should fail
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_admin_sessions_with_auth() {
    let db = setup_test_db();
    let app = build_test_router(db);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/sessions")
                .header("Authorization", "Bearer test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_admin_users_endpoint() {
    let db = setup_test_db();
    let app = build_test_router(db);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/users")
                .header("Authorization", "Bearer test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json.is_array());
}

#[tokio::test]
async fn test_admin_session_messages() {
    let db = setup_test_db();

    // Create a user and session first
    let user = create_test_user(&db, "test-external-id");
    let session = create_test_session(&db, &user.id, "test", "channel-123", "test-persona");

    // Add a message
    let session_repo = beacon_gateway::db::SessionRepo::new(db.clone());
    session_repo
        .add_message(&session.id, beacon_gateway::db::MessageRole::User, "Hello")
        .unwrap();

    let app = build_test_router(db);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/api/admin/sessions/{}/messages", session.id))
                .header("Authorization", "Bearer test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["content"], "Hello");
}
