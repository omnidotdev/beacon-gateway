//! Audio playback to speakers

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, StreamConfig};

use crate::{Error, Result};

/// Sample rate for playback (matches common TTS output)
const PLAYBACK_SAMPLE_RATE: u32 = 24000;

/// Plays audio to the default output device
pub struct AudioPlayback {
    #[allow(dead_code)]
    device: Device,
    config: StreamConfig,
}

impl AudioPlayback {
    /// Create a new audio playback instance
    ///
    /// # Errors
    ///
    /// Returns error if audio device cannot be opened
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Audio("no output device available".to_string()))?;

        let supported_config = device
            .supported_output_configs()
            .map_err(|e| Error::Audio(e.to_string()))?
            .find(|c| {
                c.channels() == 1
                    && c.min_sample_rate() <= SampleRate(PLAYBACK_SAMPLE_RATE)
                    && c.max_sample_rate() >= SampleRate(PLAYBACK_SAMPLE_RATE)
            })
            .or_else(|| {
                // Fallback: try stereo
                device.supported_output_configs().ok()?.find(|c| {
                    c.channels() == 2
                        && c.min_sample_rate() <= SampleRate(PLAYBACK_SAMPLE_RATE)
                        && c.max_sample_rate() >= SampleRate(PLAYBACK_SAMPLE_RATE)
                })
            })
            .ok_or_else(|| Error::Audio("no suitable output config found".to_string()))?;

        let config = supported_config
            .with_sample_rate(SampleRate(PLAYBACK_SAMPLE_RATE))
            .config();

        tracing::debug!(
            device = device.name().unwrap_or_default(),
            sample_rate = PLAYBACK_SAMPLE_RATE,
            channels = config.channels,
            "audio playback initialized"
        );

        Ok(Self { device, config })
    }

    /// Play audio samples (f32 format)
    ///
    /// # Errors
    ///
    /// Returns error if playback fails
    #[allow(clippy::unused_async)]
    pub async fn play(&mut self, samples: Vec<f32>) -> Result<()> {
        self.play_samples_blocking(samples)
    }

    /// Play audio from MP3 bytes
    ///
    /// # Errors
    ///
    /// Returns error if decoding or playback fails
    #[allow(clippy::unused_async)]
    pub async fn play_mp3(&mut self, mp3_data: &[u8]) -> Result<()> {
        let samples = decode_mp3(mp3_data)?;
        self.play_samples_blocking(samples)
    }

    /// Play samples in a blocking manner
    fn play_samples_blocking(&self, samples: Vec<f32>) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Audio("no output device".to_string()))?;

        let config = self.config.clone();
        let channels = config.channels as usize;

        let samples = Arc::new(Mutex::new(samples));
        let position = Arc::new(Mutex::new(0usize));
        let finished = Arc::new(Mutex::new(false));
        let finished_clone = Arc::clone(&finished);

        let samples_clone = Arc::clone(&samples);
        let position_clone = Arc::clone(&position);

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let samples = samples_clone.lock().unwrap();
                    let mut pos = position_clone.lock().unwrap();

                    for frame in data.chunks_mut(channels) {
                        let sample = if *pos < samples.len() {
                            samples[*pos]
                        } else {
                            *finished_clone.lock().unwrap() = true;
                            0.0
                        };

                        for out in frame.iter_mut() {
                            *out = sample;
                        }

                        if *pos < samples.len() {
                            *pos += 1;
                        }
                    }
                },
                |err| {
                    tracing::error!(error = %err, "audio playback error");
                },
                None,
            )
            .map_err(|e| Error::Audio(e.to_string()))?;

        stream.play().map_err(|e| Error::Audio(e.to_string()))?;

        // Wait for playback to finish
        let sample_count = samples.lock().unwrap().len();
        let duration_ms = (sample_count as u64 * 1000) / u64::from(PLAYBACK_SAMPLE_RATE);

        // Poll for completion with timeout
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(duration_ms + 500);

        while !*finished.lock().unwrap() {
            if start.elapsed() > timeout {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Small delay to ensure audio finishes
        std::thread::sleep(std::time::Duration::from_millis(100));

        drop(stream);
        tracing::debug!(samples = sample_count, "playback complete");

        Ok(())
    }
}

/// Decode MP3 bytes to f32 samples
fn decode_mp3(mp3_data: &[u8]) -> Result<Vec<f32>> {
    let mut decoder = minimp3::Decoder::new(Cursor::new(mp3_data));
    let mut samples = Vec::new();

    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                // Convert i16 samples to f32 and handle stereo to mono
                let frame_samples: Vec<f32> = if frame.channels == 2 {
                    // Stereo: average channels
                    frame
                        .data
                        .chunks(2)
                        .map(|chunk| {
                            let left = f32::from(chunk[0]) / 32768.0;
                            let right =
                                f32::from(chunk.get(1).copied().unwrap_or(chunk[0])) / 32768.0;
                            f32::midpoint(left, right)
                        })
                        .collect()
                } else {
                    // Mono
                    frame.data.iter().map(|&s| f32::from(s) / 32768.0).collect()
                };

                samples.extend(frame_samples);
            }
            Err(minimp3::Error::Eof) => break,
            Err(e) => return Err(Error::Audio(format!("MP3 decode error: {e}"))),
        }
    }

    Ok(samples)
}
