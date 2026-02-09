//! Voice pipeline integration tests
//!
//! Tests voice components without requiring audio hardware

use beacon_gateway::voice::{DetectorState, WakeWordDetector, SAMPLE_RATE, samples_to_wav};
use std::io::Cursor;

mod common;

/// Generate sine wave audio samples
fn generate_sine_samples(frequency: f32, duration_secs: f32, amplitude: f32) -> Vec<f32> {
    let num_samples = (SAMPLE_RATE as f32 * duration_secs) as usize;
    (0..num_samples)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin()
        })
        .collect()
}

/// Generate silence
fn generate_silence(duration_secs: f32) -> Vec<f32> {
    let num_samples = (SAMPLE_RATE as f32 * duration_secs) as usize;
    vec![0.0; num_samples]
}

#[test]
fn test_wake_word_detector_creation() {
    let detector = WakeWordDetector::new(vec!["hey orin".to_string()]).unwrap();

    assert_eq!(detector.state(), DetectorState::Idle);
    assert_eq!(detector.wake_words(), &["hey orin"]);
    assert!(!detector.is_activated());
    assert!(!detector.is_listening());
}

#[test]
fn test_wake_word_detector_multiple_words() {
    let detector = WakeWordDetector::new(vec![
        "hey orin".to_string(),
        "orin".to_string(),
        "hello assistant".to_string(),
    ])
    .unwrap();

    assert_eq!(detector.wake_words().len(), 3);
}

#[test]
fn test_wake_word_normalization() {
    let detector = WakeWordDetector::new(vec![
        "  Hey ORIN  ".to_string(),
        "HELLO".to_string(),
    ])
    .unwrap();

    // Should be normalized to lowercase and trimmed
    assert_eq!(detector.wake_words(), &["hey orin", "hello"]);
}

#[test]
fn test_wake_word_check() {
    let mut detector = WakeWordDetector::new(vec!["hey orin".to_string()]).unwrap();

    // No wake word
    assert!(!detector.check_wake_word("hello world"));
    assert_eq!(detector.state(), DetectorState::Idle);

    // Wake word present
    assert!(detector.check_wake_word("Hey Orin, what time is it?"));
    assert_eq!(detector.state(), DetectorState::Activated);
    assert!(detector.is_activated());
}

#[test]
fn test_wake_word_case_insensitive() {
    let mut detector = WakeWordDetector::new(vec!["hey orin".to_string()]).unwrap();

    assert!(detector.check_wake_word("HEY ORIN"));
    detector.reset();

    assert!(detector.check_wake_word("HeY oRiN"));
    detector.reset();

    assert!(detector.check_wake_word("hey orin"));
}

#[test]
fn test_detector_reset() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    // Activate
    detector.check_wake_word("orin");
    assert!(detector.is_activated());

    // Reset
    detector.reset();
    assert_eq!(detector.state(), DetectorState::Idle);
    assert!(!detector.is_activated());
}

#[test]
fn test_speech_activity_detection() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    // Silent samples - should not trigger
    let silence = generate_silence(0.1);
    assert!(!detector.process(&silence));
    assert_eq!(detector.state(), DetectorState::Idle);

    // Loud samples - should start listening
    let speech = generate_sine_samples(440.0, 0.5, 0.3);
    detector.process(&speech);
    assert_eq!(detector.state(), DetectorState::Listening);

    // More speech followed by silence should complete the segment
    let more_speech = generate_sine_samples(440.0, 0.3, 0.3);
    detector.process(&more_speech);

    let silence = generate_silence(0.6);
    let complete = detector.process(&silence);
    assert!(complete); // Speech segment complete
}

#[test]
fn test_speech_buffer_accumulation() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    let chunk1 = generate_sine_samples(440.0, 0.1, 0.3);
    detector.process(&chunk1);

    let chunk2 = generate_sine_samples(440.0, 0.1, 0.3);
    detector.process(&chunk2);

    // Buffer should contain both chunks
    let buffer = detector.speech_buffer();
    assert_eq!(buffer.len(), chunk1.len() + chunk2.len());
}

#[test]
fn test_take_speech_buffer() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    let speech = generate_sine_samples(440.0, 0.1, 0.3);
    detector.process(&speech);

    let taken = detector.take_speech_buffer();
    assert_eq!(taken.len(), speech.len());

    // Buffer should be empty after take
    assert!(detector.speech_buffer().is_empty());
}

#[test]
fn test_samples_to_wav() {
    let samples = generate_sine_samples(440.0, 0.1, 0.5);
    let wav_data = samples_to_wav(&samples, SAMPLE_RATE).unwrap();

    // Check WAV header magic
    assert_eq!(&wav_data[0..4], b"RIFF");
    assert_eq!(&wav_data[8..12], b"WAVE");

    // WAV should have reasonable size
    assert!(wav_data.len() > 44); // WAV header is 44 bytes
}

#[test]
fn test_wav_roundtrip() {
    let original_samples: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25];
    let wav_data = samples_to_wav(&original_samples, SAMPLE_RATE).unwrap();

    // Read WAV back
    let cursor = Cursor::new(wav_data);
    let mut reader = hound::WavReader::new(cursor).unwrap();

    let spec = reader.spec();
    assert_eq!(spec.sample_rate, SAMPLE_RATE);
    assert_eq!(spec.channels, 1);

    // Read samples back
    let read_samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
    assert_eq!(read_samples.len(), original_samples.len());
}

#[test]
fn test_activated_state_accumulates() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    // Activate detector
    detector.check_wake_word("orin");
    assert!(detector.is_activated());

    // Further speech should accumulate
    let speech = generate_sine_samples(440.0, 0.3, 0.3);
    detector.process(&speech);
    assert_eq!(detector.speech_buffer().len(), speech.len());

    // More speech
    let more = generate_sine_samples(440.0, 0.2, 0.3);
    detector.process(&more);
    assert_eq!(detector.speech_buffer().len(), speech.len() + more.len());
}

#[test]
fn test_utterance_complete_detection() {
    let mut detector = WakeWordDetector::new(vec!["orin".to_string()]).unwrap();

    // Activate
    detector.check_wake_word("orin");

    // Add enough speech
    let speech = generate_sine_samples(440.0, 0.5, 0.3);
    detector.process(&speech);

    // Not complete yet (no silence)
    assert!(!detector.is_utterance_complete());

    // Add silence
    let silence = generate_silence(0.6);
    detector.process(&silence);

    // Now complete
    assert!(detector.is_utterance_complete());
}

#[test]
fn test_context_builder_formatting() {
    use beacon_gateway::context::{BuiltContext, ContextMessage};

    // Test BuiltContext directly
    let context = BuiltContext {
        persona_prompt: None,
        knowledge_context: String::new(),
        system_context: "User prefers dark mode".to_string(),
        messages: vec![
            ContextMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            ContextMessage {
                role: "assistant".to_string(),
                content: "Hi there!".to_string(),
            },
        ],
        estimated_tokens: 100,
    };

    let prompt = context.format_prompt("What's the weather?");

    // Should contain user context
    assert!(prompt.contains("<user-context>"));
    assert!(prompt.contains("dark mode"));

    // Should contain conversation history
    assert!(prompt.contains("<conversation-history>"));
    assert!(prompt.contains("Hello"));
    assert!(prompt.contains("Hi there!"));

    // Should contain current message
    assert!(prompt.contains("What's the weather?"));
}
