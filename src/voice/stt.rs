//! Speech-to-text (STT) processing

use crate::{Error, Result};

/// Response from OpenAI Whisper transcription API
#[derive(serde::Deserialize)]
struct WhisperResponse {
    text: String,
}

/// Response from Deepgram transcription API
#[derive(serde::Deserialize)]
struct DeepgramResponse {
    results: DeepgramResults,
}

#[derive(serde::Deserialize)]
struct DeepgramResults {
    channels: Vec<DeepgramChannel>,
}

#[derive(serde::Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(serde::Deserialize)]
struct DeepgramAlternative {
    transcript: String,
}

/// STT provider backend
#[derive(Clone, Copy, Debug)]
enum SttProvider {
    Whisper,
    Deepgram,
}

/// Transcribes speech to text
pub struct SpeechToText {
    client: reqwest::Client,
    api_key: String,
    model: String,
    provider: SttProvider,
}

impl SpeechToText {
    /// Create a new STT instance using `OpenAI` Whisper
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_whisper(api_key: String, model: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "OpenAI API key required for Whisper".to_string(),
            ));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            provider: SttProvider::Whisper,
        })
    }

    /// Create a new STT instance using Deepgram
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_deepgram(api_key: String, model: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "Deepgram API key required".to_string(),
            ));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            provider: SttProvider::Deepgram,
        })
    }

    /// Transcribe audio to text
    ///
    /// # Arguments
    ///
    /// * `audio` - WAV audio bytes
    ///
    /// # Errors
    ///
    /// Returns error if transcription fails
    pub async fn transcribe(&self, audio: &[u8]) -> Result<String> {
        match self.provider {
            SttProvider::Whisper => self.transcribe_whisper(audio).await,
            SttProvider::Deepgram => self.transcribe_deepgram(audio).await,
        }
    }

    /// Transcribe using OpenAI Whisper
    async fn transcribe_whisper(&self, audio: &[u8]) -> Result<String> {
        tracing::debug!(audio_bytes = audio.len(), "starting Whisper transcription");

        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(audio.to_vec())
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| Error::Stt(e.to_string()))?,
            )
            .text("model", self.model.clone());

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Whisper request failed");
                e
            })?;

        let status = response.status();
        tracing::debug!(status = %status, "received response");

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Whisper API error");
            return Err(Error::Stt(format!("Whisper API error {status}: {body}")));
        }

        let result: WhisperResponse = response.json().await.map_err(|e| {
            tracing::error!(error = %e, "failed to parse response");
            e
        })?;

        tracing::info!(transcript = %result.text, "transcription complete");
        Ok(result.text)
    }

    /// Transcribe using Deepgram
    async fn transcribe_deepgram(&self, audio: &[u8]) -> Result<String> {
        tracing::debug!(audio_bytes = audio.len(), "starting Deepgram transcription");

        let url = format!(
            "https://api.deepgram.com/v1/listen?model={}&punctuate=true",
            self.model
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", "audio/wav")
            .body(audio.to_vec())
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Deepgram request failed");
                e
            })?;

        let status = response.status();
        tracing::debug!(status = %status, "received response");

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Deepgram API error");
            return Err(Error::Stt(format!("Deepgram API error {status}: {body}")));
        }

        let result: DeepgramResponse = response.json().await.map_err(|e| {
            tracing::error!(error = %e, "failed to parse Deepgram response");
            e
        })?;

        let transcript = result
            .results
            .channels
            .first()
            .and_then(|c| c.alternatives.first())
            .map(|a| a.transcript.clone())
            .unwrap_or_default();

        tracing::info!(transcript = %transcript, "transcription complete");
        Ok(transcript)
    }
}
