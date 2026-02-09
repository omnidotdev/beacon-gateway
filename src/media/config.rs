//! Configuration for media understanding

use serde::{Deserialize, Serialize};

/// Top-level media configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Enable media understanding
    pub enabled: bool,
    /// Maximum file size in bytes (default 10MB)
    pub max_file_size: usize,
    /// OpenAI Vision configuration
    pub openai: OpenAIMediaConfig,
    /// Whisper transcription configuration
    pub whisper: WhisperConfig,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_file_size: 10 * 1024 * 1024, // 10MB
            openai: OpenAIMediaConfig::default(),
            whisper: WhisperConfig::default(),
        }
    }
}

/// OpenAI Vision provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAIMediaConfig {
    /// Enable OpenAI Vision
    pub enabled: bool,
    /// Model to use (default gpt-4o)
    pub model: String,
    /// Max tokens for response
    pub max_tokens: u32,
    /// Image detail level: "auto", "low", or "high"
    pub detail: String,
}

impl Default for OpenAIMediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "gpt-4o".to_string(),
            max_tokens: 300,
            detail: "auto".to_string(),
        }
    }
}

/// Whisper transcription configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WhisperConfig {
    /// Enable Whisper transcription
    pub enabled: bool,
    /// Model to use (default whisper-1)
    pub model: String,
    /// Language hint (ISO 639-1, e.g. "en")
    pub language: Option<String>,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "whisper-1".to_string(),
            language: None,
        }
    }
}
