//! Node registry types for multi-device dispatch

use serde::{Deserialize, Serialize};

/// A connected node (device) with declared capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSession {
    pub node_id: String,
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub device_family: Option<String>,
    pub caps: Vec<String>,
    pub commands: Vec<String>,
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

/// Request to invoke a command on a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeRequest {
    pub node_id: String,
    pub command: String,
    pub params: serde_json::Value,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    pub idempotency_key: Option<String>,
}

fn default_timeout() -> u64 {
    30_000
}

/// Result from a node invocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeResult {
    pub ok: bool,
    pub payload: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Registration message from a connecting node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistration {
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub device_family: Option<String>,
    pub caps: Vec<String>,
    pub commands: Vec<String>,
}
