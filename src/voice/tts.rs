//! Text-to-speech (TTS) processing

use crate::{Error, Result};

/// TTS provider backend
#[derive(Clone, Copy, Debug)]
enum TtsProvider {
    OpenAI,
    ElevenLabs,
}

/// Synthesizes speech from text
pub struct TextToSpeech {
    client: reqwest::Client,
    api_key: String,
    voice: String,
    speed: f32,
    model: String,
    provider: TtsProvider,
}

impl TextToSpeech {
    /// Create a new TTS instance using `OpenAI`
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_openai(api_key: String, voice: String, speed: f32) -> Result<Self> {
        Self::new_openai_with_model(api_key, voice, speed, "tts-1".to_string())
    }

    /// Create a new TTS instance using `OpenAI` with custom model
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_openai_with_model(
        api_key: String,
        voice: String,
        speed: f32,
        model: String,
    ) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config("OpenAI API key required for TTS".to_string()));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            voice,
            speed,
            model,
            provider: TtsProvider::OpenAI,
        })
    }

    /// Create a new TTS instance using ElevenLabs
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_elevenlabs(api_key: String, voice_id: String) -> Result<Self> {
        Self::new_elevenlabs_with_model(api_key, voice_id, "eleven_monolingual_v1".to_string())
    }

    /// Create a new TTS instance using ElevenLabs with custom model
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new_elevenlabs_with_model(
        api_key: String,
        voice_id: String,
        model: String,
    ) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "ElevenLabs API key required for TTS".to_string(),
            ));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            voice: voice_id,
            speed: 1.0, // ElevenLabs doesn't use speed in the same way
            model,
            provider: TtsProvider::ElevenLabs,
        })
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    ///
    /// * `text` - Text to synthesize
    ///
    /// # Returns
    ///
    /// Audio bytes (MP3 format)
    ///
    /// # Errors
    ///
    /// Returns error if synthesis fails
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        match self.provider {
            TtsProvider::OpenAI => self.synthesize_openai(text).await,
            TtsProvider::ElevenLabs => self.synthesize_elevenlabs(text).await,
        }
    }

    /// Synthesize using OpenAI TTS
    async fn synthesize_openai(&self, text: &str) -> Result<Vec<u8>> {
        #[derive(serde::Serialize)]
        struct TtsRequest<'a> {
            model: &'a str,
            input: &'a str,
            voice: &'a str,
            speed: f32,
        }

        let request = TtsRequest {
            model: &self.model,
            input: text,
            voice: &self.voice,
            speed: self.speed,
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Tts(format!("OpenAI TTS error {status}: {body}")));
        }

        let audio = response.bytes().await?;
        Ok(audio.to_vec())
    }

    /// Synthesize using ElevenLabs TTS
    async fn synthesize_elevenlabs(&self, text: &str) -> Result<Vec<u8>> {
        #[derive(serde::Serialize)]
        struct ElevenLabsRequest<'a> {
            text: &'a str,
            model_id: &'a str,
        }

        let url = format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}",
            self.voice
        );

        let request = ElevenLabsRequest {
            text,
            model_id: &self.model,
        };

        let response = self
            .client
            .post(&url)
            .header("xi-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Tts(format!("ElevenLabs TTS error {status}: {body}")));
        }

        let audio = response.bytes().await?;
        Ok(audio.to_vec())
    }
}
