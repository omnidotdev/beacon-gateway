//! Audio capture from microphone

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};

use crate::{Error, Result};

/// Sample rate for audio capture (16kHz for speech)
pub const SAMPLE_RATE: u32 = 16000;

/// Captures audio from the default input device
pub struct AudioCapture {
    #[allow(dead_code)]
    device: Device,
    config: StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<Stream>,
}

impl AudioCapture {
    /// Create a new audio capture instance
    ///
    /// # Errors
    ///
    /// Returns error if audio device cannot be opened
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or_else(|| Error::Audio("no input device available".to_string()))?;

        let supported_config = device
            .supported_input_configs()
            .map_err(|e| Error::Audio(e.to_string()))?
            .find(|c| {
                c.channels() == 1
                    && c.min_sample_rate() <= SampleRate(SAMPLE_RATE)
                    && c.max_sample_rate() >= SampleRate(SAMPLE_RATE)
            })
            .ok_or_else(|| Error::Audio("no suitable audio config found".to_string()))?;

        let config = supported_config
            .with_sample_rate(SampleRate(SAMPLE_RATE))
            .config();

        tracing::debug!(
            device = device.name().unwrap_or_default(),
            sample_rate = SAMPLE_RATE,
            channels = config.channels,
            "audio capture initialized"
        );

        Ok(Self {
            device,
            config,
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
        })
    }

    /// Start capturing audio
    ///
    /// # Errors
    ///
    /// Returns error if capture fails
    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }

        let buffer = Arc::clone(&self.buffer);
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| Error::Audio("no input device".to_string()))?;

        let config = self.config.clone();

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend_from_slice(data);
                    }
                },
                |err| {
                    tracing::error!(error = %err, "audio capture error");
                },
                None,
            )
            .map_err(|e| Error::Audio(e.to_string()))?;

        stream.play().map_err(|e| Error::Audio(e.to_string()))?;
        self.stream = Some(stream);

        tracing::debug!("audio capture started");
        Ok(())
    }

    /// Stop capturing audio
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
            tracing::debug!("audio capture stopped");
        }
    }

    /// Get captured audio buffer and clear it
    ///
    /// Returns the audio samples captured since last call
    #[must_use]
    pub fn take_buffer(&self) -> Vec<f32> {
        self.buffer
            .lock()
            .map(|mut buf| std::mem::take(&mut *buf))
            .unwrap_or_default()
    }

    /// Get captured audio buffer without clearing
    #[must_use]
    pub fn peek_buffer(&self) -> Vec<f32> {
        self.buffer
            .lock()
            .map(|buf| buf.clone())
            .unwrap_or_default()
    }

    /// Clear the audio buffer
    pub fn clear_buffer(&self) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
    }

    /// Check if currently capturing
    #[must_use]
    pub const fn is_capturing(&self) -> bool {
        self.stream.is_some()
    }

    /// Get the sample rate
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
}

/// Convert f32 samples to WAV bytes for STT APIs
///
/// # Errors
///
/// Returns error if WAV encoding fails
pub fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).map_err(|e| Error::Audio(e.to_string()))?;

        for &sample in samples {
            // Convert f32 [-1.0, 1.0] to i16
            #[allow(clippy::cast_possible_truncation)]
            let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            writer
                .write_sample(sample_i16)
                .map_err(|e| Error::Audio(e.to_string()))?;
        }

        writer.finalize().map_err(|e| Error::Audio(e.to_string()))?;
    }

    Ok(cursor.into_inner())
}
