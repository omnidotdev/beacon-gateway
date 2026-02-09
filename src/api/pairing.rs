//! Device pairing API endpoints
//!
//! Provides endpoints for pairing new devices with the gateway:
//! - POST /api/pair/request - Generate a pairing code
//! - POST /api/pair/confirm - Complete pairing with code + device public key
//! - GET /api/pair/pending - List pending pairing requests
//! - DELETE /api/devices/{id} - Revoke a paired device

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::security::auth::{AuthChallenge, PairingRequest};
use crate::security::{DeviceIdentity, DeviceManager, PairedDevice, TrustLevel};

/// Shared state for pairing endpoints
pub struct PairingState {
    /// Device manager for database operations
    pub device_manager: DeviceManager,

    /// Gateway's own identity
    pub gateway_identity: DeviceIdentity,

    /// Pending pairing requests (in-memory, expire after 10 min)
    pub pending_requests: RwLock<HashMap<String, PairingRequest>>,

    /// Active challenges for WebSocket auth
    pub challenges: RwLock<HashMap<String, AuthChallenge>>,
}

impl PairingState {
    /// Create new pairing state
    #[must_use]
    pub fn new(device_manager: DeviceManager, gateway_identity: DeviceIdentity) -> Self {
        Self {
            device_manager,
            gateway_identity,
            pending_requests: RwLock::new(HashMap::new()),
            challenges: RwLock::new(HashMap::new()),
        }
    }

    /// Clean up expired requests and challenges
    pub async fn cleanup_expired(&self) {
        // Clean pending requests
        {
            let mut requests = self.pending_requests.write().await;
            requests.retain(|_, req| !req.is_expired());
        }

        // Clean challenges
        {
            let mut challenges = self.challenges.write().await;
            challenges.retain(|_, ch| !ch.is_expired());
        }
    }
}

/// Build pairing router
pub fn router(state: Arc<PairingState>) -> Router {
    Router::new()
        .route("/request", post(request_pairing))
        .route("/confirm", post(confirm_pairing))
        .route("/pending", get(list_pending))
        .route("/challenge", post(create_challenge))
        .route("/gateway", get(get_gateway_info))
        .with_state(state)
}

/// Build devices router
pub fn devices_router(state: Arc<PairingState>) -> Router {
    Router::new()
        .route("/", get(list_devices))
        .route("/{device_id}", get(get_device))
        .route("/{device_id}", delete(revoke_device))
        .route("/{device_id}/trust", post(update_trust))
        .with_state(state)
}

// === Request/Response types ===

/// Request body for pairing request
#[derive(Debug, Deserialize)]
pub struct PairingRequestBody {
    /// Optional device name
    pub device_name: Option<String>,
}

/// Response for pairing request
#[derive(Debug, Serialize)]
pub struct PairingRequestResponse {
    /// Request ID (for tracking)
    pub request_id: String,

    /// 6-digit pairing code to display
    pub code: String,

    /// Seconds until code expires
    pub expires_in: i64,
}

/// Request body for confirming pairing
#[derive(Debug, Deserialize)]
pub struct ConfirmPairingBody {
    /// Pairing code entered by user
    pub code: String,

    /// Device's public key (base64)
    pub public_key: String,

    /// Device ID (derived from public key)
    pub device_id: String,

    /// Device name
    pub device_name: String,

    /// Platform (e.g., "linux-x86_64", "macos-aarch64")
    pub platform: Option<String>,
}

/// Response for successful pairing
#[derive(Debug, Serialize)]
pub struct ConfirmPairingResponse {
    /// Confirmation of pairing
    pub success: bool,

    /// The paired device info
    pub device: PairedDeviceInfo,

    /// Gateway's public key (for mutual authentication)
    pub gateway_public_key: String,

    /// Gateway's device ID
    pub gateway_device_id: String,
}

/// Simplified device info for API responses
#[derive(Debug, Serialize)]
pub struct PairedDeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub trust_level: String,
    pub paired_at: String,
    pub last_seen: String,
}

impl From<PairedDevice> for PairedDeviceInfo {
    fn from(d: PairedDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            platform: d.platform,
            trust_level: d.trust_level.to_string(),
            paired_at: d.paired_at.to_rfc3339(),
            last_seen: d.last_seen.to_rfc3339(),
        }
    }
}

/// Response for challenge request
#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    /// Challenge ID
    pub challenge_id: String,

    /// Nonce to sign
    pub nonce: String,

    /// Seconds until challenge expires
    pub expires_in: i64,
}

/// Gateway info response
#[derive(Debug, Serialize)]
pub struct GatewayInfoResponse {
    /// Gateway's device ID
    pub device_id: String,

    /// Gateway's public key (base64)
    pub public_key: String,

    /// Gateway name
    pub name: String,

    /// Gateway version
    pub version: String,
}

/// Request body for updating trust level
#[derive(Debug, Deserialize)]
pub struct UpdateTrustBody {
    pub trust_level: String,
}

// === Handlers ===

/// Request a new pairing code
async fn request_pairing(
    State(state): State<Arc<PairingState>>,
    Json(body): Json<PairingRequestBody>,
) -> impl IntoResponse {
    // Clean up expired requests first
    state.cleanup_expired().await;

    // Generate new pairing request
    let mut request = PairingRequest::generate();
    request.device_name = body.device_name;

    let response = PairingRequestResponse {
        request_id: request.id.clone(),
        code: request.code.clone(),
        expires_in: (request.expires_at - chrono::Utc::now()).num_seconds(),
    };

    // Store pending request
    {
        let mut requests = state.pending_requests.write().await;
        requests.insert(request.id.clone(), request);
    }

    tracing::info!(code = %response.code, "generated pairing code");
    (StatusCode::CREATED, Json(response))
}

/// Confirm pairing with code and device public key
async fn confirm_pairing(
    State(state): State<Arc<PairingState>>,
    Json(body): Json<ConfirmPairingBody>,
) -> impl IntoResponse {
    // Find matching pending request by code
    let request = {
        let requests = state.pending_requests.read().await;
        requests
            .values()
            .find(|r| r.verify_code(&body.code))
            .cloned()
    };

    let Some(request) = request else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid or expired pairing code"})),
        )
            .into_response();
    };

    // Verify device ID matches public key
    // (The device ID should be a hash of the public key)
    // For now, trust the client-provided device_id

    // Register the device
    let device = match state.device_manager.register(
        &body.device_id,
        &body.public_key,
        &body.device_name,
        body.platform.as_deref(),
        TrustLevel::Paired,
    ) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Remove the used pairing request
    {
        let mut requests = state.pending_requests.write().await;
        requests.remove(&request.id);
    }

    let response = ConfirmPairingResponse {
        success: true,
        device: device.into(),
        gateway_public_key: state.gateway_identity.public_key.clone(),
        gateway_device_id: state.gateway_identity.device_id.clone(),
    };

    tracing::info!(device_id = %body.device_id, device_name = %body.device_name, "device paired");
    (StatusCode::OK, Json(response)).into_response()
}

/// List pending pairing requests (admin only)
async fn list_pending(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
    state.cleanup_expired().await;

    let requests = state.pending_requests.read().await;
    let pending: Vec<_> = requests
        .values()
        .map(|r| PairingRequestResponse {
            request_id: r.id.clone(),
            code: r.code.clone(),
            expires_in: (r.expires_at - chrono::Utc::now()).num_seconds(),
        })
        .collect();

    Json(pending)
}

/// Create an authentication challenge for WebSocket
async fn create_challenge(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
    state.cleanup_expired().await;

    let challenge = AuthChallenge::generate();
    let challenge_id = uuid::Uuid::new_v4().to_string();

    let response = ChallengeResponse {
        challenge_id: challenge_id.clone(),
        nonce: challenge.nonce.clone(),
        expires_in: (challenge.expires_at - chrono::Utc::now()).num_seconds(),
    };

    // Store challenge
    {
        let mut challenges = state.challenges.write().await;
        challenges.insert(challenge_id, challenge);
    }

    (StatusCode::CREATED, Json(response))
}

/// Get gateway info
async fn get_gateway_info(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
    Json(GatewayInfoResponse {
        device_id: state.gateway_identity.device_id.clone(),
        public_key: state.gateway_identity.public_key.clone(),
        name: state.gateway_identity.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// List all paired devices
async fn list_devices(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
    match state.device_manager.list() {
        Ok(devices) => {
            let infos: Vec<PairedDeviceInfo> = devices.into_iter().map(Into::into).collect();
            Ok(Json(infos))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Get a specific device
async fn get_device(
    State(state): State<Arc<PairingState>>,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    match state.device_manager.get(&device_id) {
        Ok(Some(device)) => Ok(Json(PairedDeviceInfo::from(device))),
        Ok(None) => Err((StatusCode::NOT_FOUND, "device not found".to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Revoke a paired device
async fn revoke_device(
    State(state): State<Arc<PairingState>>,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    match state.device_manager.remove(&device_id) {
        Ok(true) => {
            tracing::info!(device_id = %device_id, "device revoked");
            StatusCode::NO_CONTENT
        }
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Update device trust level
async fn update_trust(
    State(state): State<Arc<PairingState>>,
    Path(device_id): Path<String>,
    Json(body): Json<UpdateTrustBody>,
) -> impl IntoResponse {
    let trust_level = TrustLevel::from_str(&body.trust_level);

    match state.device_manager.update_trust_level(&device_id, trust_level) {
        Ok(()) => {
            tracing::info!(device_id = %device_id, trust_level = %trust_level, "trust level updated");
            StatusCode::OK
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup() -> Arc<PairingState> {
        let pool = init_memory().unwrap();
        let device_manager = DeviceManager::new(pool);
        let gateway_identity = DeviceIdentity::generate("test-gateway");
        Arc::new(PairingState::new(device_manager, gateway_identity))
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let state = setup();

        // Add an expired request (manually set expiry)
        let mut request = PairingRequest::generate();
        request.expires_at = chrono::Utc::now() - chrono::Duration::minutes(1);

        {
            let mut requests = state.pending_requests.write().await;
            requests.insert(request.id.clone(), request);
        }

        // Should have 1 request
        assert_eq!(state.pending_requests.read().await.len(), 1);

        // Cleanup should remove it
        state.cleanup_expired().await;
        assert_eq!(state.pending_requests.read().await.len(), 0);
    }
}
