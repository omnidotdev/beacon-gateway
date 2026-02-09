//! OpenAI Vision provider for image understanding

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::media::{MediaAnalysis, MediaConfig, MediaProvider};
use crate::{Error, Result};

/// OpenAI Vision provider
pub struct OpenAIVisionProvider {
    client: Client,
    api_key: String,
    model: String,
    detail: String,
    max_tokens: u32,
}

impl OpenAIVisionProvider {
    /// Create a new OpenAI vision provider
    #[must_use]
    pub fn new(api_key: String, config: &MediaConfig) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: config.openai.model.clone(),
            detail: config.openai.detail.clone(),
            max_tokens: config.openai.max_tokens,
        }
    }

    /// Check if MIME type is a supported image format
    fn is_supported_image(mime_type: &str) -> bool {
        matches!(
            mime_type,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        )
    }
}

#[async_trait]
impl MediaProvider for OpenAIVisionProvider {
    fn supports(&self, mime_type: &str) -> bool {
        Self::is_supported_image(mime_type)
    }

    async fn process(&self, data: &[u8], mime_type: &str) -> Result<MediaAnalysis> {
        let base64_data = base64::engine::general_purpose::STANDARD.encode(data);
        let data_url = format!("data:{mime_type};base64,{base64_data}");

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![
                    ContentPart::Text {
                        text: "Describe this image concisely. Focus on the main subject and any text visible.".to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: data_url,
                            detail: Some(self.detail.clone()),
                        },
                    },
                ],
            }],
            max_tokens: Some(self.max_tokens),
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Media(format!("OpenAI request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Media(format!("OpenAI API error: {status} - {body}")));
        }

        let result: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| Error::Media(format!("Failed to parse OpenAI response: {e}")))?;

        let description = result
            .choices
            .first()
            .and_then(|c| c.message.content.clone());

        Ok(MediaAnalysis {
            description,
            transcript: None,
            metadata: serde_json::json!({
                "provider": "openai",
                "model": self.model,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "openai-vision"
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: Vec<ContentPart>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_image_types() {
        assert!(OpenAIVisionProvider::is_supported_image("image/png"));
        assert!(OpenAIVisionProvider::is_supported_image("image/jpeg"));
        assert!(OpenAIVisionProvider::is_supported_image("image/gif"));
        assert!(OpenAIVisionProvider::is_supported_image("image/webp"));
        assert!(!OpenAIVisionProvider::is_supported_image("audio/mp3"));
        assert!(!OpenAIVisionProvider::is_supported_image("video/mp4"));
    }
}
