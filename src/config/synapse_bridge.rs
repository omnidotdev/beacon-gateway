//! Translate beacon Config into synapse_config::Config for embedded mode
//!
//! Auto-configures LLM, STT, and TTS providers from available API keys

use secrecy::SecretString;
use synapse_config::llm::{LlmProviderConfig, LlmProviderType, ModelConfig};
use synapse_config::stt::{SttProviderConfig, SttProviderType};
use synapse_config::tts::{TtsProviderConfig, TtsProviderType};

/// Build a `synapse_config::Config` from beacon's config
///
/// Maps API keys to LLM/STT/TTS providers:
/// - Anthropic key → LLM anthropic provider
/// - OpenAI key → LLM openai + STT whisper + TTS openai
/// - OpenRouter key → LLM openrouter (via openai-compatible)
/// - ElevenLabs key → TTS elevenlabs
/// - Deepgram key → STT deepgram
pub fn build_synapse_config(config: &super::Config) -> synapse_config::Config {
    let mut sc = synapse_config::Config::default();

    // LLM providers
    if let Some(ref key) = config.api_keys.anthropic {
        sc.llm.providers.insert(
            "anthropic".to_string(),
            LlmProviderConfig {
                provider_type: LlmProviderType::Anthropic,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
                models: ModelConfig::default(),
                headers: vec![],
                forward_authorization: false,
                rate_limit: None,
            },
        );
    }

    if let Some(ref key) = config.api_keys.openai {
        sc.llm.providers.insert(
            "openai".to_string(),
            LlmProviderConfig {
                provider_type: LlmProviderType::Openai,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
                models: ModelConfig::default(),
                headers: vec![],
                forward_authorization: false,
                rate_limit: None,
            },
        );
    }

    if let Some(ref key) = config.api_keys.openrouter {
        sc.llm.providers.insert(
            "openrouter".to_string(),
            LlmProviderConfig {
                provider_type: LlmProviderType::Openai,
                api_key: Some(SecretString::from(key.clone())),
                base_url: "https://openrouter.ai/api/v1".parse().ok(),
                models: ModelConfig::default(),
                headers: vec![],
                forward_authorization: false,
                rate_limit: None,
            },
        );
    }

    // STT providers
    if let Some(ref key) = config.api_keys.openai {
        sc.stt.providers.insert(
            "whisper".to_string(),
            SttProviderConfig {
                provider_type: SttProviderType::Whisper,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
            },
        );
    }

    if let Some(ref key) = config.api_keys.deepgram {
        sc.stt.providers.insert(
            "deepgram".to_string(),
            SttProviderConfig {
                provider_type: SttProviderType::Deepgram,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
            },
        );
    }

    // TTS providers
    if let Some(ref key) = config.api_keys.openai {
        sc.tts.providers.insert(
            "openai".to_string(),
            TtsProviderConfig {
                provider_type: TtsProviderType::OpenaiTts,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
            },
        );
    }

    if let Some(ref key) = config.api_keys.elevenlabs {
        sc.tts.providers.insert(
            "elevenlabs".to_string(),
            TtsProviderConfig {
                provider_type: TtsProviderType::Elevenlabs,
                api_key: Some(SecretString::from(key.clone())),
                base_url: None,
            },
        );
    }

    sc
}
