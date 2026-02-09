//! Error types for Beacon gateway

use thiserror::Error;

/// Result type alias for Beacon operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in Beacon gateway
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration error
    #[error("configuration error: {0}")]
    Config(String),

    /// Persona not found
    #[error("persona not found: {0}")]
    PersonaNotFound(String),

    /// Voice processing error
    #[error("voice error: {0}")]
    Voice(String),

    /// Audio error
    #[error("audio error: {0}")]
    Audio(String),

    /// Speech-to-text error
    #[error("STT error: {0}")]
    Stt(String),

    /// Text-to-speech error
    #[error("TTS error: {0}")]
    Tts(String),

    /// Wake word detection error
    #[error("wake word error: {0}")]
    WakeWord(String),

    /// Channel error
    #[error("channel error: {0}")]
    Channel(String),

    /// Browser automation error
    #[error("browser error: {0}")]
    Browser(String),

    /// Web fetch error (SSRF protection, request failures)
    #[error("web fetch error: {0}")]
    WebFetch(String),

    /// Agent error
    #[error("agent error: {0}")]
    Agent(String),

    /// IO error
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP error
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// TOML parsing error
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Database error
    #[error("database error: {0}")]
    Database(String),

    /// `SQLite` error
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Skill error
    #[error("skill error: {0}")]
    Skill(String),

    /// Manifold error
    #[error("manifold error: {0}")]
    Manifold(String),

    /// Resource not found
    #[error("not found: {0}")]
    NotFound(String),

    /// Embedding error
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Authentication/authorization error
    #[error("auth error: {0}")]
    Auth(String),

    /// Vault/BYOK error
    #[error("vault error: {0}")]
    Vault(String),

    /// Attachment processing error
    #[error("attachment error: {0}")]
    Attachment(String),

    /// Vision API error
    #[error("vision error: {0}")]
    Vision(String),

    /// Media processing error
    #[error("media error: {0}")]
    Media(String),

    /// Link processing error
    #[error("link error: {0}")]
    Link(String),
}
