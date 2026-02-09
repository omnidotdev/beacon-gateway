//! Vision API client for image analysis
//!
//! Uses Claude's vision capabilities to describe images

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Vision client for image analysis
pub struct VisionClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

/// Anthropic message request
#[derive(Debug, Serialize)]
struct MessageRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<Message<'a>>,
}

/// A message in the request
#[derive(Debug, Serialize)]
struct Message<'a> {
    role: &'a str,
    content: Vec<ContentBlock<'a>>,
}

/// Content block (text or image)
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock<'a> {
    #[serde(rename = "text")]
    Text { text: &'a str },
    #[serde(rename = "image")]
    Image { source: ImageSource<'a> },
}

/// Image source
#[derive(Debug, Serialize)]
struct ImageSource<'a> {
    #[serde(rename = "type")]
    source_type: &'a str,
    media_type: &'a str,
    data: String,
}

/// Anthropic message response
#[derive(Debug, Deserialize)]
struct MessageResponse {
    content: Vec<ResponseContent>,
}

/// Response content block
#[derive(Debug, Deserialize)]
struct ResponseContent {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    content_type: String,
    text: Option<String>,
}

impl VisionClient {
    /// Create a new vision client
    ///
    /// # Errors
    ///
    /// Returns error if API key is missing
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::Config(
                "Anthropic API key required for vision".to_string(),
            ));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model: DEFAULT_MODEL.to_string(),
        })
    }

    /// Create with a specific model
    #[must_use]
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Describe an image
    ///
    /// # Arguments
    ///
    /// * `image_data` - Raw image bytes
    /// * `mime_type` - MIME type of the image
    ///
    /// # Errors
    ///
    /// Returns error if API call fails
    pub async fn describe_image(&self, image_data: &[u8], mime_type: &str) -> Result<String> {
        let base64_data = base64::engine::general_purpose::STANDARD.encode(image_data);

        let media_type = normalize_mime_type(mime_type);

        let request = MessageRequest {
            model: &self.model,
            max_tokens: 300,
            messages: vec![Message {
                role: "user",
                content: vec![
                    ContentBlock::Image {
                        source: ImageSource {
                            source_type: "base64",
                            media_type,
                            data: base64_data,
                        },
                    },
                    ContentBlock::Text {
                        text: "Describe this image concisely in 1-2 sentences. Focus on the main subject and any text visible.",
                    },
                ],
            }],
        };

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Vision(format!("Request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Vision(format!("API error {status}: {body}")));
        }

        let result: MessageResponse = response
            .json()
            .await
            .map_err(|e| Error::Vision(format!("Parse error: {e}")))?;

        // Extract text from response
        let description = result
            .content
            .into_iter()
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join(" ");

        if description.is_empty() {
            return Err(Error::Vision("Empty response from vision API".to_string()));
        }

        tracing::debug!(description = %description, "image described");
        Ok(description)
    }
}

/// Normalize MIME type for Anthropic API
fn normalize_mime_type(mime_type: &str) -> &'static str {
    match mime_type.to_lowercase().as_str() {
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        // jpeg, jpg, and any unknown type default to jpeg
        _ => "image/jpeg",
    }
}
