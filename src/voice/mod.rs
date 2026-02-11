//! Voice processing module
//!
//! Handles audio capture, wake word detection, and playback.
//! STT and TTS are routed through Synapse (see `daemon.rs`)

mod capture;
mod playback;
mod wake_word;

pub use capture::{AudioCapture, SAMPLE_RATE, samples_to_wav};
pub use playback::AudioPlayback;
pub use wake_word::{DetectorState, WakeWordDetector};
