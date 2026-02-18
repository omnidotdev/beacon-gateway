//! HTTP client for auto-provisioning managed API keys via Synapse API

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::Result;

/// Response from the Synapse provision-managed-key endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedKey {
    /// Raw API key (synapse_...)
    pub api_key: String,
    /// Last 4 characters of the key
    pub key_hint: String,
    /// Synapse user ID
    pub user_id: String,
    /// User's plan tier
    pub plan: String,
}

/// Request body for the provision endpoint
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRequest {
    identity_provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

/// Client for auto-provisioning managed keys via Synapse API
#[derive(Debug, Clone)]
pub struct KeyProvisioner {
    client: Client,
    base_url: String,
    gateway_secret: String,
}

impl KeyProvisioner {
    /// Create a new key provisioner
    ///
    /// # Arguments
    ///
    /// * `synapse_api_url` - Base URL for the Synapse API (e.g., `https://api.synapse.omni.dev`)
    /// * `gateway_secret` - Shared secret for `x-gateway-secret` header
    #[must_use]
    pub fn new(synapse_api_url: impl Into<String>, gateway_secret: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: synapse_api_url.into(),
            gateway_secret: gateway_secret.into(),
        }
    }

    /// Provision a managed API key for a user
    ///
    /// # Errors
    ///
    /// Returns error if the HTTP request fails or the response is invalid
    pub async fn provision(
        &self,
        identity_provider_id: &str,
        email: Option<&str>,
        name: Option<&str>,
    ) -> Result<ProvisionedKey> {
        let url = format!(
            "{}/internal/provision-managed-key",
            self.base_url.trim_end_matches('/')
        );

        let body = ProvisionRequest {
            identity_provider_id: identity_provider_id.to_string(),
            email: email.map(String::from),
            name: name.map(String::from),
        };

        let response = self
            .client
            .post(&url)
            .header("x-gateway-secret", &self.gateway_secret)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::Error::Vault(format!("synapse provision request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(crate::Error::Vault(format!(
                "synapse provision failed: {status} - {body}"
            )));
        }

        let key: ProvisionedKey = response
            .json()
            .await
            .map_err(|e| crate::Error::Vault(format!("invalid provision response: {e}")))?;

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provision_request_serialization() {
        let body = ProvisionRequest {
            identity_provider_id: "test-id-123".to_string(),
            email: Some("test@example.com".to_string()),
            name: None,
        };

        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("identityProviderId"));
        assert!(json.contains("test-id-123"));
        assert!(json.contains("email"));
        assert!(!json.contains("name"));
    }

    #[test]
    fn test_provisioned_key_deserialization() {
        let json = r#"{
            "apiKey": "synapse_abc123",
            "keyHint": "c123",
            "userId": "user-456",
            "plan": "free"
        }"#;

        let key: ProvisionedKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.api_key, "synapse_abc123");
        assert_eq!(key.key_hint, "c123");
        assert_eq!(key.user_id, "user-456");
        assert_eq!(key.plan, "free");
    }
}
