//! Media provider implementations
//!
//! Available providers:
//! - OpenAI Vision for image understanding
//! - Whisper for audio transcription

mod openai;
mod whisper;

pub use openai::OpenAIVisionProvider;
pub use whisper::WhisperProvider;
