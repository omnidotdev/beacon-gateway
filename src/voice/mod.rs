//! Voice processing module
//!
//! Handles:
//! - Audio capture from microphone
//! - Wake word detection
//! - Speech-to-text (STT)
//! - Text-to-speech (TTS)
//! - Audio playback
//!
//! # Performance TODOs
//!
//! TODO: Local wake word detection (Porcupine, Vosk) to avoid STT round-trip
//! TODO: Streaming STT for lower latency (`OpenAI` Realtime API or Whisper streaming)
//! TODO: Streaming TTS for faster first-byte response
//! TODO: Response caching for common queries
//! TODO: Local TTS fallback (Piper, Coqui) for offline mode

mod capture;
mod playback;
mod stt;
mod tts;
mod wake_word;

pub use capture::{AudioCapture, SAMPLE_RATE, samples_to_wav};
pub use playback::AudioPlayback;
pub use stt::SpeechToText;
pub use tts::TextToSpeech;
pub use wake_word::{DetectorState, WakeWordDetector};
