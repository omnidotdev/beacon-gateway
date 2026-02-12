//! Attachment processing for multimodal messages
//!
//! Processes images, audio, and other attachments to augment message context

mod vision;

use std::sync::Arc;

use synapse_client::SynapseClient;

use crate::channels::{Attachment, AttachmentKind};
use crate::Result;

pub use vision::VisionClient;

/// Processes attachments and returns text descriptions/transcriptions
pub struct AttachmentProcessor {
    /// Vision client for image analysis
    vision: Option<Arc<VisionClient>>,
    /// Synapse client for audio transcription
    synapse: Option<Arc<SynapseClient>>,
    /// STT model identifier for Synapse transcription
    stt_model: String,
    /// HTTP client for downloading attachments
    client: reqwest::Client,
}

impl AttachmentProcessor {
    /// Create a new attachment processor
    #[must_use]
    pub fn new(
        vision: Option<Arc<VisionClient>>,
        synapse: Option<Arc<SynapseClient>>,
        stt_model: String,
    ) -> Self {
        Self {
            vision,
            synapse,
            stt_model,
            client: reqwest::Client::new(),
        }
    }

    /// Process all attachments and return augmented text
    ///
    /// # Errors
    ///
    /// Returns error if attachment processing fails
    pub async fn process_attachments(&self, attachments: &[Attachment]) -> Result<String> {
        if attachments.is_empty() {
            return Ok(String::new());
        }

        let mut parts = Vec::new();

        for attachment in attachments {
            let description = self.process_single(attachment).await;
            parts.push(description);
        }

        Ok(parts.join("\n"))
    }

    /// Process a single attachment
    async fn process_single(&self, attachment: &Attachment) -> String {
        match attachment.kind {
            AttachmentKind::Image => self.process_image(attachment).await,
            AttachmentKind::Audio => self.process_audio(attachment).await,
            AttachmentKind::Video => self.process_video(attachment),
            AttachmentKind::File => self.process_file(attachment),
        }
    }

    /// Process an image attachment using vision
    async fn process_image(&self, attachment: &Attachment) -> String {
        let Some(vision) = &self.vision else {
            return format!(
                "[Image: {}]",
                attachment.filename.as_deref().unwrap_or("image")
            );
        };

        // Get image data
        let image_data = match self.get_attachment_data(attachment).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "failed to download image");
                return format!(
                    "[Image: {} (could not download)]",
                    attachment.filename.as_deref().unwrap_or("image")
                );
            }
        };

        // Analyze with vision
        match vision.describe_image(&image_data, &attachment.mime_type).await {
            Ok(description) => {
                format!(
                    "[Image: {}]\n{}",
                    attachment.filename.as_deref().unwrap_or("image"),
                    description
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "vision analysis failed");
                format!(
                    "[Image: {} (analysis failed)]",
                    attachment.filename.as_deref().unwrap_or("image")
                )
            }
        }
    }

    /// Process an audio attachment using Synapse STT
    async fn process_audio(&self, attachment: &Attachment) -> String {
        let Some(synapse) = &self.synapse else {
            return format!(
                "[Audio: {}]",
                attachment.filename.as_deref().unwrap_or("audio")
            );
        };

        // Get audio data
        let audio_data = match self.get_attachment_data(attachment).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "failed to download audio");
                return format!(
                    "[Audio: {} (could not download)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                );
            }
        };

        // Convert to WAV if needed and transcribe
        let wav_data = match convert_to_wav(&audio_data, &attachment.mime_type) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "audio conversion failed");
                return format!(
                    "[Audio: {} (unsupported format)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                );
            }
        };

        let filename = attachment.filename.as_deref().unwrap_or("audio.wav");
        match synapse.transcribe(wav_data.into(), filename, &self.stt_model).await {
            Ok(result) => {
                format!(
                    "[Audio transcription: {}]\n\"{}\"",
                    attachment.filename.as_deref().unwrap_or("audio"),
                    result.text
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "transcription failed");
                format!(
                    "[Audio: {} (transcription failed)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                )
            }
        }
    }

    /// Process a video attachment (metadata only for now)
    #[allow(clippy::unused_self)]
    fn process_video(&self, attachment: &Attachment) -> String {
        format!(
            "[Video: {}]",
            attachment.filename.as_deref().unwrap_or("video")
        )
    }

    /// Process a generic file attachment
    #[allow(clippy::unused_self)]
    fn process_file(&self, attachment: &Attachment) -> String {
        format!(
            "[File: {} ({})]",
            attachment.filename.as_deref().unwrap_or("file"),
            attachment.mime_type
        )
    }

    /// Get attachment data from URL or inline data
    async fn get_attachment_data(&self, attachment: &Attachment) -> Result<Vec<u8>> {
        // If we have inline data, use it
        if let Some(data) = &attachment.data {
            return Ok(data.clone());
        }

        // Otherwise download from URL
        let url = attachment
            .url
            .as_ref()
            .ok_or_else(|| crate::Error::Attachment("No URL or data for attachment".to_string()))?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| crate::Error::Attachment(format!("Download failed: {e}")))?;

        if !response.status().is_success() {
            return Err(crate::Error::Attachment(format!(
                "Download failed: {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| crate::Error::Attachment(format!("Read failed: {e}")))?;

        Ok(bytes.to_vec())
    }
}

/// Convert audio to WAV format for STT
fn convert_to_wav(data: &[u8], mime_type: &str) -> Result<Vec<u8>> {
    match mime_type {
        "audio/wav" | "audio/wave" | "audio/x-wav" => Ok(data.to_vec()),
        "audio/mpeg" | "audio/mp3" => convert_mp3_to_wav(data),
        "audio/ogg" | "audio/opus" => {
            // OGG/Opus conversion would require additional dependencies
            Err(crate::Error::Attachment(
                "OGG/Opus conversion not yet supported".to_string(),
            ))
        }
        _ => {
            // Try to use as-is and let Whisper handle it
            Ok(data.to_vec())
        }
    }
}

/// Convert MP3 to WAV using minimp3
#[allow(clippy::cast_sign_loss)]
fn convert_mp3_to_wav(mp3_data: &[u8]) -> Result<Vec<u8>> {
    use crate::voice::samples_to_wav;

    let mut decoder = minimp3::Decoder::new(mp3_data);
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 16000_u32;

    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                sample_rate = frame.sample_rate as u32;
                // Convert to mono f32 if stereo
                if frame.channels == 2 {
                    for chunk in frame.data.chunks(2) {
                        let mono = f32::midpoint(f32::from(chunk[0]), f32::from(chunk[1])) / 32768.0;
                        samples.push(mono);
                    }
                } else {
                    for &s in &frame.data {
                        samples.push(f32::from(s) / 32768.0);
                    }
                }
            }
            Err(minimp3::Error::Eof) => break,
            Err(e) => {
                return Err(crate::Error::Attachment(format!("MP3 decode error: {e}")));
            }
        }
    }

    // Resample to 16kHz if needed
    let resampled = if sample_rate == 16000 {
        samples
    } else {
        resample_audio(&samples, sample_rate, 16000)?
    };

    samples_to_wav(&resampled, 16000)
}

/// Resample audio using rubato
#[allow(clippy::cast_possible_truncation)]
fn resample_audio(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    use rubato::{FftFixedIn, Resampler};

    let chunk_size = 1024;
    let sub_chunks = 2;

    let mut resampler =
        FftFixedIn::<f64>::new(from_rate as usize, to_rate as usize, chunk_size, sub_chunks, 1)
            .map_err(|e| crate::Error::Attachment(format!("Resampler init failed: {e}")))?;

    // Convert to f64
    let input: Vec<f64> = samples.iter().map(|&s| f64::from(s)).collect();

    let mut output = Vec::new();

    for chunk in input.chunks(chunk_size) {
        if chunk.len() == chunk_size {
            let result = resampler
                .process(&[chunk.to_vec()], None)
                .map_err(|e| crate::Error::Attachment(format!("Resample failed: {e}")))?;
            output.extend_from_slice(&result[0]);
        }
    }

    // Convert back to f32
    Ok(output.iter().map(|&s| s as f32).collect())
}
