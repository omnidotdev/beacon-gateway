//! Whisper provider for audio transcription

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;

use crate::media::{MediaAnalysis, MediaConfig, MediaProvider};
use crate::{Error, Result};

/// Whisper transcription provider
pub struct WhisperProvider {
    client: Client,
    api_key: String,
    model: String,
    language: Option<String>,
}

impl WhisperProvider {
    /// Create a new Whisper provider
    #[must_use]
    pub fn new(api_key: String, config: &MediaConfig) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: config.whisper.model.clone(),
            language: config.whisper.language.clone(),
        }
    }

    /// Check if MIME type is a supported audio format
    fn is_supported_audio(mime_type: &str) -> bool {
        matches!(
            mime_type,
            "audio/mpeg"
                | "audio/mp3"
                | "audio/mp4"
                | "audio/m4a"
                | "audio/wav"
                | "audio/webm"
                | "audio/ogg"
                | "audio/flac"
        )
    }

    /// Get file extension for MIME type
    fn extension_for_mime(mime_type: &str) -> &'static str {
        match mime_type {
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/mp4" | "audio/m4a" => "m4a",
            "audio/wav" => "wav",
            "audio/webm" => "webm",
            "audio/ogg" => "ogg",
            "audio/flac" => "flac",
            _ => "mp3",
        }
    }
}

#[async_trait]
impl MediaProvider for WhisperProvider {
    fn supports(&self, mime_type: &str) -> bool {
        Self::is_supported_audio(mime_type)
    }

    async fn process(&self, data: &[u8], mime_type: &str) -> Result<MediaAnalysis> {
        let extension = Self::extension_for_mime(mime_type);
        let filename = format!("audio.{extension}");

        let part = Part::bytes(data.to_vec())
            .file_name(filename)
            .mime_str(mime_type)
            .map_err(|e| Error::Media(format!("Invalid MIME type: {e}")))?;

        let mut form = Form::new()
            .text("model", self.model.clone())
            .part("file", part);

        if let Some(ref lang) = self.language {
            form = form.text("language", lang.clone());
        }

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| Error::Media(format!("Whisper request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Media(format!("Whisper API error: {status} - {body}")));
        }

        let result: TranscriptionResponse = response
            .json()
            .await
            .map_err(|e| Error::Media(format!("Failed to parse Whisper response: {e}")))?;

        Ok(MediaAnalysis {
            description: None,
            transcript: Some(result.text),
            metadata: serde_json::json!({
                "provider": "whisper",
                "model": self.model,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "whisper"
    }
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_audio_types() {
        assert!(WhisperProvider::is_supported_audio("audio/mpeg"));
        assert!(WhisperProvider::is_supported_audio("audio/mp3"));
        assert!(WhisperProvider::is_supported_audio("audio/wav"));
        assert!(WhisperProvider::is_supported_audio("audio/ogg"));
        assert!(WhisperProvider::is_supported_audio("audio/flac"));
        assert!(!WhisperProvider::is_supported_audio("image/png"));
        assert!(!WhisperProvider::is_supported_audio("video/mp4"));
    }

    #[test]
    fn test_extension_for_mime() {
        assert_eq!(WhisperProvider::extension_for_mime("audio/mpeg"), "mp3");
        assert_eq!(WhisperProvider::extension_for_mime("audio/mp3"), "mp3");
        assert_eq!(WhisperProvider::extension_for_mime("audio/wav"), "wav");
        assert_eq!(WhisperProvider::extension_for_mime("audio/ogg"), "ogg");
        assert_eq!(WhisperProvider::extension_for_mime("audio/flac"), "flac");
    }
}
