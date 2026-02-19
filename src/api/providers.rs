//! Provider configuration API for BYOK (Bring Your Own Key)
//!
//! Lists available LLM providers and their status. Key management (adding/removing
//! provider keys) is handled via the Synapse dashboard at `/dashboard/provider-keys`.
//! Keys are resolved per-user at request time via the Synapse API.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Available LLM provider types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// `OpenAI` API (GPT models)
    Openai,
    /// Anthropic API (Claude models)
    Anthropic,
    /// `OpenRouter` (aggregated access to multiple providers)
    Openrouter,
    /// Omni Credits (pay-per-use via Synapse router)
    OmniCredits,
}

/// Provider configuration status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Provider is configured and ready
    Configured,
    /// Provider is not configured (no API key)
    NotConfigured,
    /// Provider is coming soon
    ComingSoon,
    /// Provider configuration is invalid
    Invalid,
}

/// Provider information returned to clients
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: ProviderType,
    pub name: String,
    pub description: String,
    pub status: ProviderStatus,
    /// Whether this provider is currently active
    pub active: bool,
    /// URL for getting an API key (for BYOK providers)
    pub api_key_url: Option<String>,
    /// Whether this provider is coming soon
    pub coming_soon: bool,
    /// Features available with this provider
    pub features: Vec<String>,
}

/// All providers response
#[derive(Debug, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderInfo>,
    pub active_provider: Option<ProviderType>,
}

/// Extract user ID from JWT in the Authorization header
async fn extract_user_id(headers: &HeaderMap, state: &ApiState) -> Option<String> {
    let Some(jwt_cache) = state.jwt_cache.as_ref() else {
        tracing::warn!("BYOK auth failed: no JWT cache (AUTH_BASE_URL or BEACON_SERVICE_KEY not set)");
        return None;
    };

    let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        tracing::warn!("BYOK auth failed: no Authorization header in request");
        return None;
    };

    let Some(token) = auth.strip_prefix("Bearer ") else {
        tracing::warn!("BYOK auth failed: Authorization header is not Bearer scheme");
        return None;
    };

    match jwt_cache.validate(token).await {
        Ok(claims) => Some(claims.sub),
        Err(e) => {
            tracing::warn!(error = %e, "BYOK auth failed: JWT validation error");
            None
        }
    }
}

/// Resolve provider status for a specific provider
///
/// Checks per-user vault keys first, then falls back to env-level config
fn provider_status(
    provider_str: &str,
    user_configured: &[String],
    state: &ApiState,
) -> (ProviderStatus, bool) {
    if user_configured.contains(&provider_str.to_string()) {
        return (ProviderStatus::Configured, true);
    }

    let env_active = state
        .model_info
        .as_ref()
        .is_some_and(|m| m.provider == provider_str);

    if env_active {
        (ProviderStatus::Configured, true)
    } else {
        (ProviderStatus::NotConfigured, false)
    }
}

/// Get all available providers and their status
async fn list_providers(
    headers: HeaderMap,
    State(state): State<Arc<ApiState>>,
) -> Json<ProvidersResponse> {
    // Check per-user configured providers if authenticated
    let user_id = extract_user_id(&headers, &state).await;
    let user_configured = if let Some(ref uid) = user_id {
        if let Some(resolver) = &state.key_resolver {
            resolver.list_configured(uid).await
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let (openai_status, openai_active) = provider_status("openai", &user_configured, &state);
    let (anthropic_status, anthropic_active) =
        provider_status("anthropic", &user_configured, &state);
    let (openrouter_status, openrouter_active) =
        provider_status("openrouter", &user_configured, &state);

    let providers = vec![
        ProviderInfo {
            id: ProviderType::Openai,
            name: "OpenAI".to_string(),
            description: "GPT-4o, GPT-4, and other OpenAI models".to_string(),
            status: openai_status,
            active: openai_active,
            api_key_url: Some("https://platform.openai.com/api-keys".to_string()),
            coming_soon: false,
            features: vec![
                "Chat completions".to_string(),
                "Whisper STT".to_string(),
                "TTS".to_string(),
            ],
        },
        ProviderInfo {
            id: ProviderType::Anthropic,
            name: "Anthropic".to_string(),
            description: "Claude Opus 4.5, Sonnet 4.5, and other Claude models".to_string(),
            status: anthropic_status,
            active: anthropic_active,
            api_key_url: Some("https://console.anthropic.com/settings/keys".to_string()),
            coming_soon: false,
            features: vec![
                "Chat completions".to_string(),
                "Tool use".to_string(),
                "Extended context".to_string(),
            ],
        },
        ProviderInfo {
            id: ProviderType::Openrouter,
            name: "OpenRouter".to_string(),
            description: "Access 500+ models from all major providers with one API key".to_string(),
            status: openrouter_status,
            active: openrouter_active,
            api_key_url: Some("https://openrouter.ai/keys".to_string()),
            coming_soon: false,
            features: vec![
                "500+ models".to_string(),
                "Unified billing".to_string(),
                "Automatic fallbacks".to_string(),
            ],
        },
        {
            let synapse_available = state.synapse.is_some();
            let provisioner_available = state.key_provisioner.is_some();
            let has_cached_key = user_configured.contains(&"omni_credits".to_string());

            let (omni_status, omni_active, omni_coming_soon) = if synapse_available && provisioner_available {
                if user_id.is_some() || has_cached_key {
                    // Authenticated user with provisioner = fully configured
                    (ProviderStatus::Configured, true, false)
                } else {
                    // Not authenticated, sign in to activate
                    (ProviderStatus::NotConfigured, false, false)
                }
            } else if synapse_available {
                // Synapse but no provisioner
                (ProviderStatus::NotConfigured, false, false)
            } else {
                (ProviderStatus::ComingSoon, false, true)
            };

            ProviderInfo {
                id: ProviderType::OmniCredits,
                name: "Omni Credits".to_string(),
                description: "Omni's AI router with smart model selection and MCP support. No API keys needed".to_string(),
                status: omni_status,
                active: omni_active,
                api_key_url: None,
                coming_soon: omni_coming_soon,
                features: vec![
                    "Smart routing".to_string(),
                    "MCP server aggregation".to_string(),
                    "Cost optimization".to_string(),
                    "Tool discovery".to_string(),
                ],
            }
        },
    ];

    // Determine active provider: BYOK keys override Omni Credits.
    // Mirror resolve_user_synapse priority: anthropic → openai → openrouter → omni_credits → env fallback
    let has_byok = user_configured.iter().any(|p| p != "omni_credits");
    let active_provider = if has_byok {
        if user_configured.contains(&"anthropic".to_string()) {
            Some(ProviderType::Anthropic)
        } else if user_configured.contains(&"openai".to_string()) {
            Some(ProviderType::Openai)
        } else {
            Some(ProviderType::Openrouter)
        }
    } else if user_configured.contains(&"omni_credits".to_string())
        || (state.key_provisioner.is_some() && user_id.is_some())
    {
        Some(ProviderType::OmniCredits)
    } else {
        state.model_info.as_ref().map(|m| match m.provider.as_str() {
            "anthropic" => ProviderType::Anthropic,
            _ => ProviderType::Openai,
        })
    };

    Json(ProvidersResponse {
        providers,
        active_provider,
    })
}

/// Create the providers router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(list_providers))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_serialization() {
        let provider = ProviderType::Openai;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"openai\"");

        let provider = ProviderType::OmniCredits;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"omni_credits\"");
    }
}
