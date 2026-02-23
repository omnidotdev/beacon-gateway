//! TOML configuration file loading
//!
//! Supports `~/.config/omni/beacon/config.toml` as a persistent config source.
//! All fields are optional — the file is a partial overlay on top of defaults.

use std::path::PathBuf;

use serde::Deserialize;

/// Top-level TOML configuration file schema
#[derive(Debug, Default, Deserialize)]
pub struct BeaconConfigFile {
    /// Persona identifier (e.g. "orin")
    #[serde(default)]
    pub persona: Option<String>,

    /// LLM configuration
    #[serde(default)]
    pub llm: LlmFileConfig,

    /// Voice/audio configuration
    #[serde(default)]
    pub voice: VoiceFileConfig,

    /// API keys for external services
    #[serde(default)]
    pub api_keys: ApiKeysFileConfig,

    /// Channel configuration (Discord, Slack, etc)
    #[serde(default)]
    pub channels: ChannelsFileConfig,

    /// Server/runtime configuration
    #[serde(default)]
    pub server: ServerFileConfig,

    /// Skills system configuration
    #[serde(default)]
    pub skills: SkillsFileConfig,
}

/// LLM-related configuration
#[derive(Debug, Default, Deserialize)]
pub struct LlmFileConfig {
    /// Model identifier (e.g. "claude-sonnet-4-20250514")
    pub model: Option<String>,

    /// Preferred provider ("anthropic", "openai", "openrouter")
    pub provider: Option<String>,
}

/// Voice processing configuration
#[derive(Debug, Default, Deserialize)]
pub struct VoiceFileConfig {
    /// Enable voice input/output
    pub enabled: Option<bool>,

    /// STT model (e.g. "whisper-1")
    pub stt_model: Option<String>,

    /// TTS model (e.g. "tts-1")
    pub tts_model: Option<String>,

    /// TTS voice identifier (e.g. "alloy")
    pub tts_voice: Option<String>,

    /// TTS speed multiplier
    pub tts_speed: Option<f64>,
}

/// API keys configuration
#[derive(Debug, Default, Deserialize)]
pub struct ApiKeysFileConfig {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub openrouter: Option<String>,
    pub elevenlabs: Option<String>,
    pub deepgram: Option<String>,
    pub discord: Option<String>,
    pub slack: Option<String>,
    pub telegram: Option<String>,
}

/// Channel-specific configuration
#[derive(Debug, Default, Deserialize)]
pub struct ChannelsFileConfig {
    #[serde(default)]
    pub discord: Option<ChannelToggle>,

    #[serde(default)]
    pub slack: Option<ChannelToggle>,

    #[serde(default)]
    pub telegram: Option<ChannelToggle>,

    #[serde(default)]
    pub imessage: Option<IMessageFileConfig>,
}

/// Simple channel toggle (token lives in api_keys)
#[derive(Debug, Default, Deserialize)]
pub struct ChannelToggle {
    pub enabled: Option<bool>,
}

/// iMessage-specific channel config
#[derive(Debug, Default, Deserialize)]
pub struct IMessageFileConfig {
    pub enabled: Option<bool>,
    pub cli_path: Option<String>,
    pub db_path: Option<String>,
    pub region: Option<String>,
    pub service: Option<String>,
}

/// Server/runtime configuration
#[derive(Debug, Default, Deserialize)]
pub struct ServerFileConfig {
    /// API server port
    pub port: Option<u16>,

    /// Synapse AI router URL
    pub synapse_url: Option<String>,

    /// Cloud mode toggle
    pub cloud_mode: Option<bool>,
}

/// Skills system configuration
#[derive(Debug, Default, Deserialize)]
pub struct SkillsFileConfig {
    /// Path to managed skills directory
    pub managed_dir: Option<String>,
    /// Max skills in prompt
    pub max_skills_in_prompt: Option<usize>,
    /// Max total chars from skills in prompt
    pub max_skills_prompt_chars: Option<usize>,
    /// Max bytes per individual skill file
    pub max_skill_file_bytes: Option<usize>,
    /// Additional skill directories to scan
    pub extra_dirs: Option<Vec<String>>,
    /// Personal agent skills directory
    pub personal_dir: Option<String>,
    /// Bundled skill allowlist (empty = all)
    pub allow_bundled: Option<Vec<String>>,
}

/// Load the TOML config file from the standard path
///
/// Returns `BeaconConfigFile::default()` if the file doesn't exist or can't be parsed.
pub fn load_config_file() -> BeaconConfigFile {
    let Some(path) = config_file_path() else {
        return BeaconConfigFile::default();
    };

    if !path.exists() {
        return BeaconConfigFile::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => {
                tracing::info!(path = %path.display(), "loaded config file");
                config
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to parse config file, using defaults"
                );
                BeaconConfigFile::default()
            }
        },
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to read config file"
            );
            BeaconConfigFile::default()
        }
    }
}

/// Return the config file path: `~/.config/omni/beacon/config.toml`
pub fn config_file_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| {
        d.config_dir()
            .join("omni")
            .join("beacon")
            .join("config.toml")
    })
}
