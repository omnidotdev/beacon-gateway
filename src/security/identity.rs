//! Device identity management using Ed25519 cryptography
//!
//! Each gateway instance has a unique device identity consisting of an Ed25519
//! keypair. The device ID is derived from the public key using SHA-256

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Length of device ID in hex characters (32 = 128 bits)
const DEVICE_ID_LENGTH: usize = 32;

/// Device identity stored on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    /// Unique device identifier (truncated SHA-256 of public key)
    pub device_id: String,

    /// Ed25519 public key (base64 encoded)
    pub public_key: String,

    /// Ed25519 private key (base64 encoded)
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_key: Option<String>,

    /// Human-readable device name
    pub name: String,

    /// Platform identifier (e.g., "linux-x86_64", "macos-aarch64")
    pub platform: String,

    /// When the identity was created
    pub created_at: DateTime<Utc>,
}

impl DeviceIdentity {
    /// Generate a new device identity with a random keypair
    #[must_use]
    pub fn generate(name: &str) -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let public_key_bytes = verifying_key.as_bytes();
        let device_id = compute_device_id(public_key_bytes);

        let platform = format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );

        Self {
            device_id,
            public_key: base64_encode(public_key_bytes),
            secret_key: Some(base64_encode(signing_key.as_bytes())),
            name: name.to_string(),
            platform,
            created_at: Utc::now(),
        }
    }

    /// Load identity from a file, or create a new one if it doesn't exist
    ///
    /// # Errors
    ///
    /// Returns error if file operations fail or JSON is invalid
    pub fn load_or_create(path: &Path, default_name: &str) -> Result<Self> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let identity: Self = serde_json::from_str(&content)
                .map_err(|e| Error::Config(format!("invalid device identity: {e}")))?;
            tracing::debug!(device_id = %identity.device_id, "loaded device identity");
            Ok(identity)
        } else {
            let identity = Self::generate(default_name);

            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            let content = serde_json::to_string_pretty(&identity)
                .map_err(|e| Error::Config(format!("failed to serialize identity: {e}")))?;
            fs::write(path, content)?;

            tracing::info!(device_id = %identity.device_id, "created new device identity");
            Ok(identity)
        }
    }

    /// Get the default identity file path
    ///
    /// Returns `~/.local/share/omni/beacon/identity/device.json`
    #[must_use]
    pub fn default_path() -> PathBuf {
        directories::BaseDirs::new().map_or_else(
            || PathBuf::from(".local/share/omni/beacon/identity/device.json"),
            |d| {
                d.data_dir()
                    .join("omni")
                    .join("beacon")
                    .join("identity")
                    .join("device.json")
            },
        )
    }

    /// Sign a payload with the device's secret key
    ///
    /// # Errors
    ///
    /// Returns error if identity has no secret key
    pub fn sign(&self, payload: &[u8]) -> Result<String> {
        let secret_key = self
            .secret_key
            .as_ref()
            .ok_or_else(|| Error::Auth("identity has no secret key".to_string()))?;

        let key_bytes = base64_decode(secret_key)?;
        let signing_key = SigningKey::try_from(key_bytes.as_slice())
            .map_err(|e| Error::Auth(format!("invalid secret key: {e}")))?;

        let signature = signing_key.sign(payload);
        Ok(base64_encode(&signature.to_bytes()))
    }

    /// Verify a signature against this identity's public key
    ///
    /// # Errors
    ///
    /// Returns error if public key is invalid
    pub fn verify(&self, payload: &[u8], signature: &str) -> Result<bool> {
        let public_key_bytes = base64_decode(&self.public_key)?;
        let verifying_key = VerifyingKey::try_from(public_key_bytes.as_slice())
            .map_err(|e| Error::Auth(format!("invalid public key: {e}")))?;

        let sig_bytes = base64_decode(signature)?;
        let signature = Signature::try_from(sig_bytes.as_slice())
            .map_err(|e| Error::Auth(format!("invalid signature format: {e}")))?;

        Ok(verifying_key.verify(payload, &signature).is_ok())
    }

    /// Create a public-only copy of this identity (for sharing)
    #[must_use]
    pub fn public_only(&self) -> Self {
        Self {
            device_id: self.device_id.clone(),
            public_key: self.public_key.clone(),
            secret_key: None,
            name: self.name.clone(),
            platform: self.platform.clone(),
            created_at: self.created_at,
        }
    }

    /// Check if this identity has a secret key
    #[must_use]
    pub const fn has_secret_key(&self) -> bool {
        self.secret_key.is_some()
    }

    /// Get the short device ID (first 8 characters)
    #[must_use]
    pub fn short_id(&self) -> &str {
        &self.device_id[..8.min(self.device_id.len())]
    }
}

/// Verify a signature from a public key (without a full identity)
///
/// # Errors
///
/// Returns error if public key or signature format is invalid
pub fn verify_signature(public_key: &str, payload: &[u8], signature: &str) -> Result<bool> {
    let public_key_bytes = base64_decode(public_key)?;
    let verifying_key = VerifyingKey::try_from(public_key_bytes.as_slice())
        .map_err(|e| Error::Auth(format!("invalid public key: {e}")))?;

    let sig_bytes = base64_decode(signature)?;
    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| Error::Auth(format!("invalid signature format: {e}")))?;

    Ok(verifying_key.verify(payload, &signature).is_ok())
}

/// Compute device ID from public key bytes
fn compute_device_id(public_key: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(public_key);
    let hash = hasher.finalize();

    // Take first DEVICE_ID_LENGTH/2 bytes (each byte = 2 hex chars)
    hex::encode(&hash[..DEVICE_ID_LENGTH / 2])
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(data: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| Error::Auth(format!("invalid base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_identity() {
        let identity = DeviceIdentity::generate("test-device");

        assert_eq!(identity.device_id.len(), DEVICE_ID_LENGTH);
        assert!(!identity.public_key.is_empty());
        assert!(identity.secret_key.is_some());
        assert_eq!(identity.name, "test-device");
    }

    #[test]
    fn test_sign_and_verify() {
        let identity = DeviceIdentity::generate("test");
        let payload = b"hello world";

        let signature = identity.sign(payload).unwrap();
        assert!(identity.verify(payload, &signature).unwrap());

        // Tampered payload should fail
        assert!(!identity.verify(b"tampered", &signature).unwrap());
    }

    #[test]
    fn test_public_only() {
        let identity = DeviceIdentity::generate("test");
        let public = identity.public_only();

        assert_eq!(public.device_id, identity.device_id);
        assert_eq!(public.public_key, identity.public_key);
        assert!(public.secret_key.is_none());
        assert!(!public.has_secret_key());
    }

    #[test]
    fn test_verify_signature_standalone() {
        let identity = DeviceIdentity::generate("test");
        let payload = b"test payload";

        let signature = identity.sign(payload).unwrap();
        assert!(verify_signature(&identity.public_key, payload, &signature).unwrap());
    }

    #[test]
    fn test_short_id() {
        let identity = DeviceIdentity::generate("test");
        assert_eq!(identity.short_id().len(), 8);
    }

    #[test]
    fn test_device_id_deterministic() {
        // Same public key should produce same device ID
        let identity1 = DeviceIdentity::generate("test1");
        let computed_id = compute_device_id(&base64_decode(&identity1.public_key).unwrap());
        assert_eq!(computed_id, identity1.device_id);
    }
}
