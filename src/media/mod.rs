//! Media understanding for images, audio, and video
//!
//! Provides a provider-based system for analyzing media attachments

mod config;
pub mod providers;

pub use config::MediaConfig;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;

/// Analysis result from media processing
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaAnalysis {
    /// Text description of visual content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Transcript for audio/video content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Additional metadata from the provider
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Trait for media processing providers
#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Check if this provider supports the given MIME type
    fn supports(&self, mime_type: &str) -> bool;

    /// Process media data and return analysis
    ///
    /// # Errors
    ///
    /// Returns error if processing fails
    async fn process(&self, data: &[u8], mime_type: &str) -> Result<MediaAnalysis>;

    /// Provider name for logging
    fn name(&self) -> &'static str;
}

/// Media processor with fallback chain
pub struct MediaProcessor {
    providers: Vec<Box<dyn MediaProvider>>,
    config: MediaConfig,
}

impl MediaProcessor {
    /// Create a new media processor
    #[must_use]
    pub fn new(config: MediaConfig) -> Self {
        Self {
            providers: Vec::new(),
            config,
        }
    }

    /// Add a provider to the chain
    pub fn add_provider(&mut self, provider: Box<dyn MediaProvider>) {
        self.providers.push(provider);
    }

    /// Check if media processing is enabled
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Process media through the provider chain
    ///
    /// # Errors
    ///
    /// Returns error if no provider can process the media
    pub async fn process(&self, data: &[u8], mime_type: &str) -> Result<MediaAnalysis> {
        if !self.config.enabled {
            return Ok(MediaAnalysis::default());
        }

        for provider in &self.providers {
            if provider.supports(mime_type) {
                tracing::debug!(provider = provider.name(), mime_type, "processing media");
                match provider.process(data, mime_type).await {
                    Ok(analysis) => return Ok(analysis),
                    Err(e) => {
                        tracing::warn!(
                            provider = provider.name(),
                            error = %e,
                            "provider failed, trying next"
                        );
                    }
                }
            }
        }

        Err(crate::Error::Media(format!(
            "no provider available for MIME type: {mime_type}"
        )))
    }
}
