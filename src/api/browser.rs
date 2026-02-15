//! Browser automation REST endpoints
//!
//! Expose `BrowserController` operations over HTTP so CLI and other
//! clients can delegate browser tasks to the gateway

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::tools::{BrowserController, BrowserControllerConfig};

/// Shared browser controller state
pub type SharedBrowser = Arc<Mutex<BrowserController>>;

/// Create a shared browser controller with default config
#[must_use]
pub fn default_browser() -> SharedBrowser {
    Arc::new(Mutex::new(BrowserController::new(
        BrowserControllerConfig::default(),
    )))
}

// --- Request/Response types ---

#[derive(Deserialize)]
pub struct NavigateRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct NavigateResponse {
    pub url: String,
    pub title: Option<String>,
    pub text: Option<String>,
}

#[derive(Deserialize)]
pub struct ScreenshotRequest {
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct ScreenshotResponse {
    /// Base64-encoded PNG image
    pub data: String,
    pub format: String,
}

#[derive(Deserialize)]
pub struct ClickRequest {
    pub selector: String,
}

#[derive(Deserialize)]
pub struct TypeRequest {
    pub selector: String,
    pub text: String,
}

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub script: String,
}

#[derive(Serialize)]
pub struct ExecuteResponse {
    pub result: serde_json::Value,
}

#[derive(Serialize)]
pub struct BrowserStatus {
    pub running: bool,
}

#[derive(Serialize)]
pub struct BrowserError {
    pub error: String,
}

/// Build browser automation routes
pub fn router(browser: SharedBrowser) -> Router {
    Router::new()
        .route("/navigate", post(navigate))
        .route("/screenshot", post(screenshot))
        .route("/click", post(click))
        .route("/type", post(type_text))
        .route("/execute", post(execute))
        .route("/status", get(status))
        .with_state(browser)
}

/// Ensure browser is launched, launching if needed
async fn ensure_running(browser: &BrowserController) -> Result<(), (StatusCode, Json<BrowserError>)> {
    if !browser.is_running().await {
        browser.launch().await.map_err(|e| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(BrowserError {
                    error: format!("failed to launch browser: {e}"),
                }),
            )
        })?;
    }
    Ok(())
}

/// Navigate to a URL
async fn navigate(
    State(browser): State<SharedBrowser>,
    Json(req): Json<NavigateRequest>,
) -> Result<Json<NavigateResponse>, (StatusCode, Json<BrowserError>)> {
    let browser = browser.lock().await;
    ensure_running(&browser).await?;

    let content = browser.navigate(&req.url).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(BrowserError {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(NavigateResponse {
        url: content.url,
        title: content.title,
        text: content.text,
    }))
}

/// Take a screenshot
async fn screenshot(
    State(browser): State<SharedBrowser>,
    Json(req): Json<ScreenshotRequest>,
) -> Result<Json<ScreenshotResponse>, (StatusCode, Json<BrowserError>)> {
    let browser = browser.lock().await;
    ensure_running(&browser).await?;

    let shot = browser
        .screenshot(req.url.as_deref())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(BrowserError {
                    error: e.to_string(),
                }),
            )
        })?;

    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&shot.data);

    Ok(Json(ScreenshotResponse {
        data: encoded,
        format: shot.format.to_string(),
    }))
}

/// Click an element
async fn click(
    State(browser): State<SharedBrowser>,
    Json(req): Json<ClickRequest>,
) -> Result<StatusCode, (StatusCode, Json<BrowserError>)> {
    let browser = browser.lock().await;
    ensure_running(&browser).await?;

    browser.click(&req.selector).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(BrowserError {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

/// Type text into an element
async fn type_text(
    State(browser): State<SharedBrowser>,
    Json(req): Json<TypeRequest>,
) -> Result<StatusCode, (StatusCode, Json<BrowserError>)> {
    let browser = browser.lock().await;
    ensure_running(&browser).await?;

    browser.type_text(&req.selector, &req.text).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(BrowserError {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

/// Execute JavaScript
async fn execute(
    State(browser): State<SharedBrowser>,
    Json(req): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, (StatusCode, Json<BrowserError>)> {
    let browser = browser.lock().await;
    ensure_running(&browser).await?;

    let result = browser.execute_js(&req.script).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(BrowserError {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(ExecuteResponse { result }))
}

/// Get browser status
async fn status(
    State(browser): State<SharedBrowser>,
) -> Json<BrowserStatus> {
    let browser = browser.lock().await;
    Json(BrowserStatus {
        running: browser.is_running().await,
    })
}
