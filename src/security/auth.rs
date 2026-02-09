//! Gateway authentication modes
//!
//! Provides multiple authentication modes for connecting to the gateway:
//! - Open: No authentication required
//! - Token: Bearer token authentication
//! - Password: Password-based authentication (for local access)
//! - `DeviceOnly`: Only paired devices can connect

use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Pairing code length (digits only for easy entry)
const PAIRING_CODE_LENGTH: usize = 6;

/// Pairing code expiry in minutes
const PAIRING_CODE_EXPIRY_MINUTES: i64 = 10;

/// Nonce length in bytes
const NONCE_LENGTH: usize = 32;

/// Nonce expiry in seconds
const NONCE_EXPIRY_SECONDS: i64 = 60;

/// Authentication mode for gateway access
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// No authentication required (development/trusted network)
    #[default]
    Open,

    /// Bearer token authentication
    Token,

    /// Password authentication (hashed)
    Password,

    /// Only paired devices can connect (device identity required)
    DeviceOnly,
}

impl AuthMode {
    /// Parse from string representation
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "token" => Self::Token,
            "password" => Self::Password,
            "device" | "deviceonly" | "device_only" => Self::DeviceOnly,
            _ => Self::Open,
        }
    }
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Token => write!(f, "token"),
            Self::Password => write!(f, "password"),
            Self::DeviceOnly => write!(f, "device_only"),
        }
    }
}

/// Gateway authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Authentication mode
    pub mode: AuthMode,

    /// Bearer token (for Token mode)
    pub token: Option<String>,

    /// Password hash (for Password mode, bcrypt or argon2)
    pub password_hash: Option<String>,

    /// Allow unauthenticated access from local/LAN addresses
    pub allow_local_bypass: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::Open,
            token: None,
            password_hash: None,
            allow_local_bypass: true,
        }
    }
}

impl AuthConfig {
    /// Create from environment variables
    #[must_use]
    pub fn from_env() -> Self {
        let mode = std::env::var("BEACON_AUTH_MODE")
            .map(|s| AuthMode::from_str(&s))
            .unwrap_or_default();

        let token = std::env::var("BEACON_AUTH_TOKEN").ok();
        let password_hash = std::env::var("BEACON_AUTH_PASSWORD_HASH").ok();
        let allow_local_bypass = std::env::var("BEACON_AUTH_ALLOW_LOCAL_BYPASS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        Self {
            mode,
            token,
            password_hash,
            allow_local_bypass,
        }
    }

    /// Check if an IP address should bypass authentication
    #[must_use]
    pub fn should_bypass(&self, ip: Option<IpAddr>) -> bool {
        if !self.allow_local_bypass {
            return false;
        }

        let Some(ip) = ip else {
            return false;
        };

        // Allow localhost
        if ip.is_loopback() {
            return true;
        }

        // Allow LAN addresses
        match ip {
            IpAddr::V4(v4) => {
                // 10.0.0.0/8
                if v4.octets()[0] == 10 {
                    return true;
                }
                // 172.16.0.0/12
                if v4.octets()[0] == 172 && (16..=31).contains(&v4.octets()[1]) {
                    return true;
                }
                // 192.168.0.0/16
                if v4.octets()[0] == 192 && v4.octets()[1] == 168 {
                    return true;
                }
                // 127.0.0.0/8 (loopback range)
                if v4.octets()[0] == 127 {
                    return true;
                }
            }
            IpAddr::V6(v6) => {
                // ::1 (loopback)
                if v6.is_loopback() {
                    return true;
                }
                // fe80::/10 (link-local)
                let segments = v6.segments();
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return true;
                }
            }
        }

        false
    }

    /// Verify a bearer token (timing-safe comparison)
    #[must_use]
    pub fn verify_token(&self, provided: &str) -> bool {
        let Some(expected) = &self.token else {
            return false;
        };

        constant_time_eq(expected.as_bytes(), provided.as_bytes())
    }
}

/// Pending device pairing request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Unique request ID
    pub id: String,

    /// 6-digit pairing code
    pub code: String,

    /// Device ID requesting pairing (if known)
    pub device_id: Option<String>,

    /// Device name
    pub device_name: Option<String>,

    /// When the request was created
    pub created_at: DateTime<Utc>,

    /// When the code expires
    pub expires_at: DateTime<Utc>,
}

impl PairingRequest {
    /// Generate a new pairing request with a 6-digit code
    #[must_use]
    pub fn generate() -> Self {
        let code = generate_pairing_code(PAIRING_CODE_LENGTH);
        let now = Utc::now();
        let expires_at = now + Duration::minutes(PAIRING_CODE_EXPIRY_MINUTES);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            code,
            device_id: None,
            device_name: None,
            created_at: now,
            expires_at,
        }
    }

    /// Check if the pairing code has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Verify a provided code (timing-safe)
    #[must_use]
    pub fn verify_code(&self, provided: &str) -> bool {
        if self.is_expired() {
            return false;
        }
        constant_time_eq(self.code.as_bytes(), provided.as_bytes())
    }
}

/// Challenge for WebSocket authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// Random nonce to sign
    pub nonce: String,

    /// When the challenge was created
    pub created_at: DateTime<Utc>,

    /// When the challenge expires
    pub expires_at: DateTime<Utc>,
}

impl AuthChallenge {
    /// Generate a new authentication challenge
    #[must_use]
    pub fn generate() -> Self {
        let nonce = generate_nonce(NONCE_LENGTH);
        let now = Utc::now();
        let expires_at = now + Duration::seconds(NONCE_EXPIRY_SECONDS);

        Self {
            nonce,
            created_at: now,
            expires_at,
        }
    }

    /// Check if the challenge has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Get the payload to sign (nonce as bytes)
    #[must_use]
    pub fn payload(&self) -> Vec<u8> {
        self.nonce.as_bytes().to_vec()
    }
}

/// Verify a device signature against a challenge
///
/// # Errors
///
/// Returns error if verification fails
pub fn verify_device_signature(
    public_key: &str,
    challenge: &AuthChallenge,
    signature: &str,
) -> Result<bool> {
    if challenge.is_expired() {
        return Err(Error::Auth("challenge expired".to_string()));
    }

    crate::security::verify_signature(public_key, &challenge.payload(), signature)
}

/// Generate a random numeric pairing code
fn generate_pairing_code(length: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| rng.gen_range(0..10).to_string())
        .collect()
}

/// Generate a random hex nonce
fn generate_nonce(length: usize) -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..length).map(|_| rng.r#gen()).collect();
    hex::encode(bytes)
}

/// Constant-time byte comparison to prevent timing attacks
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_mode_parsing() {
        assert_eq!(AuthMode::from_str("open"), AuthMode::Open);
        assert_eq!(AuthMode::from_str("token"), AuthMode::Token);
        assert_eq!(AuthMode::from_str("password"), AuthMode::Password);
        assert_eq!(AuthMode::from_str("device"), AuthMode::DeviceOnly);
        assert_eq!(AuthMode::from_str("deviceonly"), AuthMode::DeviceOnly);
        assert_eq!(AuthMode::from_str("device_only"), AuthMode::DeviceOnly);
        assert_eq!(AuthMode::from_str("unknown"), AuthMode::Open);
    }

    #[test]
    fn test_local_bypass() {
        let config = AuthConfig {
            allow_local_bypass: true,
            ..Default::default()
        };

        // Localhost should bypass
        assert!(config.should_bypass(Some("127.0.0.1".parse().unwrap())));
        assert!(config.should_bypass(Some("::1".parse().unwrap())));

        // LAN addresses should bypass
        assert!(config.should_bypass(Some("192.168.1.100".parse().unwrap())));
        assert!(config.should_bypass(Some("10.0.0.50".parse().unwrap())));
        assert!(config.should_bypass(Some("172.16.0.1".parse().unwrap())));

        // Public IPs should not bypass
        assert!(!config.should_bypass(Some("8.8.8.8".parse().unwrap())));
        assert!(!config.should_bypass(Some("1.1.1.1".parse().unwrap())));

        // Disabled bypass
        let strict_config = AuthConfig {
            allow_local_bypass: false,
            ..Default::default()
        };
        assert!(!strict_config.should_bypass(Some("127.0.0.1".parse().unwrap())));
    }

    #[test]
    fn test_token_verification() {
        let config = AuthConfig {
            token: Some("secret-token-123".to_string()),
            ..Default::default()
        };

        assert!(config.verify_token("secret-token-123"));
        assert!(!config.verify_token("wrong-token"));
        assert!(!config.verify_token("secret-token-12")); // Partial match
    }

    #[test]
    fn test_pairing_code_generation() {
        let code = generate_pairing_code(6);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_pairing_request() {
        let request = PairingRequest::generate();
        assert_eq!(request.code.len(), PAIRING_CODE_LENGTH);
        assert!(!request.is_expired());
        assert!(request.verify_code(&request.code));
        assert!(!request.verify_code("000000"));
    }

    #[test]
    fn test_auth_challenge() {
        let challenge = AuthChallenge::generate();
        assert_eq!(challenge.nonce.len(), NONCE_LENGTH * 2); // Hex encoding
        assert!(!challenge.is_expired());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
    }
}
