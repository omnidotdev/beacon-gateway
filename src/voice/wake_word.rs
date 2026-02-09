//! Wake word detection
//!
//! Detects wake words in audio stream to activate the assistant.
//! Uses a hybrid approach: local energy detection + cloud verification.

use crate::Result;

/// Minimum audio energy threshold to consider speech
const ENERGY_THRESHOLD: f32 = 0.03;

/// Minimum duration of speech to trigger (in samples at 16kHz)
const MIN_SPEECH_SAMPLES: usize = 4800; // 0.3 seconds

/// Silence duration to consider end of utterance (in samples)
const SILENCE_SAMPLES: usize = 8000; // 0.5 seconds

/// State of the wake word detector
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorState {
    /// Waiting for speech
    Idle,
    /// Detected potential speech, accumulating
    Listening,
    /// Wake word detected, capturing utterance
    Activated,
}

/// Detects wake words in audio
pub struct WakeWordDetector {
    wake_words: Vec<String>,
    state: DetectorState,
    speech_buffer: Vec<f32>,
    silence_counter: usize,
}

impl WakeWordDetector {
    /// Create a new wake word detector
    ///
    /// # Arguments
    ///
    /// * `wake_words` - List of wake words to detect (e.g., "hey orin")
    ///
    /// # Errors
    ///
    /// Returns error if detector cannot be initialized
    pub fn new(wake_words: Vec<String>) -> Result<Self> {
        let normalized: Vec<String> = wake_words
            .into_iter()
            .map(|w| w.to_lowercase().trim().to_string())
            .collect();

        tracing::debug!(wake_words = ?normalized, "wake word detector initialized");

        Ok(Self {
            wake_words: normalized,
            state: DetectorState::Idle,
            speech_buffer: Vec::new(),
            silence_counter: 0,
        })
    }

    /// Process audio samples and detect speech activity
    ///
    /// Returns true if speech activity is detected (not wake word yet)
    pub fn process(&mut self, samples: &[f32]) -> bool {
        let energy = calculate_energy(samples);
        let is_speech = energy > ENERGY_THRESHOLD;

        match self.state {
            DetectorState::Idle => {
                if is_speech {
                    self.state = DetectorState::Listening;
                    self.speech_buffer.clear();
                    self.speech_buffer.extend_from_slice(samples);
                    self.silence_counter = 0;
                    tracing::trace!(energy, "speech detected, listening");
                }
            }
            DetectorState::Listening => {
                self.speech_buffer.extend_from_slice(samples);

                if is_speech {
                    self.silence_counter = 0;
                } else {
                    self.silence_counter += samples.len();
                }

                tracing::trace!(
                    buffer_len = self.speech_buffer.len(),
                    silence = self.silence_counter,
                    is_speech,
                    energy,
                    "listening state"
                );

                // Check if we have enough speech followed by silence
                if self.silence_counter > SILENCE_SAMPLES
                    && self.speech_buffer.len() > MIN_SPEECH_SAMPLES
                {
                    tracing::debug!(
                        samples = self.speech_buffer.len(),
                        "speech segment complete"
                    );
                    return true;
                }

                // Timeout: too much silence without enough speech
                if self.silence_counter > SILENCE_SAMPLES * 2 {
                    tracing::trace!("timeout - resetting");
                    self.reset();
                }
            }
            DetectorState::Activated => {
                // Already activated, accumulating utterance
                self.speech_buffer.extend_from_slice(samples);

                if is_speech {
                    self.silence_counter = 0;
                } else {
                    self.silence_counter += samples.len();
                }
            }
        }

        false
    }

    /// Check if transcribed text contains a wake word
    ///
    /// Call this after STT to verify wake word presence
    pub fn check_wake_word(&mut self, transcript: &str) -> bool {
        let normalized = transcript.to_lowercase();

        for wake_word in &self.wake_words {
            if normalized.contains(wake_word) {
                tracing::info!(wake_word, transcript, "wake word detected");
                self.state = DetectorState::Activated;
                return true;
            }
        }

        // No wake word, reset
        self.reset();
        false
    }

    /// Get the accumulated speech buffer
    #[must_use]
    pub fn speech_buffer(&self) -> &[f32] {
        &self.speech_buffer
    }

    /// Take the speech buffer, clearing it
    pub fn take_speech_buffer(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.speech_buffer)
    }

    /// Check if currently activated (wake word was detected)
    #[must_use]
    pub fn is_activated(&self) -> bool {
        self.state == DetectorState::Activated
    }

    /// Check if currently listening for potential wake word
    #[must_use]
    pub fn is_listening(&self) -> bool {
        self.state == DetectorState::Listening
    }

    /// Check if utterance capture is complete (silence after speech)
    #[must_use]
    pub fn is_utterance_complete(&self) -> bool {
        self.state == DetectorState::Activated
            && self.silence_counter > SILENCE_SAMPLES
            && self.speech_buffer.len() > MIN_SPEECH_SAMPLES
    }

    /// Reset detector to idle state
    pub fn reset(&mut self) {
        self.state = DetectorState::Idle;
        self.speech_buffer.clear();
        self.silence_counter = 0;
    }

    /// Get current state
    #[must_use]
    pub const fn state(&self) -> DetectorState {
        self.state
    }

    /// Get the configured wake words
    #[must_use]
    pub fn wake_words(&self) -> &[String] {
        &self.wake_words
    }

    /// Manually activate (skip wake word detection)
    pub const fn activate(&mut self) {
        self.state = DetectorState::Activated;
        self.silence_counter = 0;
    }
}

/// Calculate RMS energy of audio samples
#[allow(clippy::cast_precision_loss)]
fn calculate_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_energy_calculation() {
        let silence = vec![0.0f32; 100];
        assert!(calculate_energy(&silence) < 0.001);

        let loud = vec![0.5f32; 100];
        assert!(calculate_energy(&loud) > 0.4);
    }

    #[test]
    fn test_wake_word_detection() {
        let mut detector = WakeWordDetector::new(vec!["hey orin".to_string()]).unwrap();

        assert!(!detector.check_wake_word("hello world"));
        assert_eq!(detector.state(), DetectorState::Idle);

        assert!(detector.check_wake_word("Hey Orin, what's up?"));
        assert_eq!(detector.state(), DetectorState::Activated);
    }
}
