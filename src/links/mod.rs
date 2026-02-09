//! Link understanding for URL preview extraction
//!
//! Detects URLs in messages and extracts Open Graph / Twitter Card metadata

mod config;
mod detector;

pub use config::LinkConfig;
pub use detector::detect_urls;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use lru::LruCache;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{Error, Result};

/// Extracted link preview metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPreview {
    /// Original URL
    pub url: String,
    /// Page title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Page description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Preview image URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Site name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// Favicon URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
}

/// Link processor with caching
pub struct LinkProcessor {
    client: Client,
    cache: Arc<Mutex<LruCache<String, LinkPreview>>>,
    config: LinkConfig,
}

impl LinkProcessor {
    /// Create a new link processor
    #[must_use]
    pub fn new(config: LinkConfig) -> Self {
        let cache_size = NonZeroUsize::new(100).expect("100 is non-zero");
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(config.timeout_secs))
                .user_agent("Mozilla/5.0 (compatible; BeaconBot/1.0)")
                .build()
                .expect("failed to build HTTP client"),
            cache: Arc::new(Mutex::new(LruCache::new(cache_size))),
            config,
        }
    }

    /// Check if link processing is enabled
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Extract previews for all URLs in a message
    ///
    /// # Errors
    ///
    /// Individual URL failures are logged but don't cause overall failure
    pub async fn process_message(&self, content: &str) -> Result<Vec<LinkPreview>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let urls = detect_urls(content);
        let urls: Vec<_> = urls.into_iter().take(self.config.max_urls).collect();

        let mut previews = Vec::new();
        for url in urls {
            match self.get_preview(&url).await {
                Ok(preview) => previews.push(preview),
                Err(e) => tracing::debug!(url, error = %e, "failed to get link preview"),
            }
        }

        Ok(previews)
    }

    /// Get preview for a single URL (with caching)
    ///
    /// # Errors
    ///
    /// Returns error if fetching or parsing fails
    pub async fn get_preview(&self, url: &str) -> Result<LinkPreview> {
        // Check cache first
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(url) {
                return Ok(cached.clone());
            }
        }

        // Fetch and parse
        let preview = self.fetch_preview(url).await?;

        // Cache the result
        {
            let mut cache = self.cache.lock().await;
            cache.put(url.to_string(), preview.clone());
        }

        Ok(preview)
    }

    /// Fetch preview metadata from URL
    async fn fetch_preview(&self, url: &str) -> Result<LinkPreview> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Link(format!("Failed to fetch URL: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Link(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| Error::Link(format!("Failed to read response: {e}")))?;

        self.parse_html(url, &html)
    }

    /// Parse HTML for Open Graph and Twitter Card metadata
    fn parse_html(&self, url: &str, html: &str) -> Result<LinkPreview> {
        use scraper::{Html, Selector};

        let document = Html::parse_document(html);

        // Helper to get meta content
        let get_meta = |property: &str| -> Option<String> {
            let selector = Selector::parse(&format!(
                r#"meta[property="{property}"], meta[name="{property}"]"#
            ))
            .ok()?;
            document
                .select(&selector)
                .next()
                .and_then(|el| el.value().attr("content"))
                .map(String::from)
        };

        // Get title
        let title = get_meta("og:title")
            .or_else(|| get_meta("twitter:title"))
            .or_else(|| {
                let selector = Selector::parse("title").ok()?;
                document
                    .select(&selector)
                    .next()
                    .map(|el| el.text().collect::<String>())
            });

        // Get description
        let description = get_meta("og:description")
            .or_else(|| get_meta("twitter:description"))
            .or_else(|| get_meta("description"));

        // Get image
        let image_url = get_meta("og:image").or_else(|| get_meta("twitter:image"));

        // Get site name
        let site_name = get_meta("og:site_name");

        // Get favicon
        let favicon_url = {
            let selector =
                Selector::parse(r#"link[rel="icon"], link[rel="shortcut icon"]"#).ok();
            selector.and_then(|s| {
                document
                    .select(&s)
                    .next()
                    .and_then(|el| el.value().attr("href"))
                    .map(|href| {
                        if href.starts_with("http") {
                            href.to_string()
                        } else if href.starts_with('/') {
                            // Make absolute URL
                            if let Ok(base) = url::Url::parse(url) {
                                format!(
                                    "{}://{}{}",
                                    base.scheme(),
                                    base.host_str().unwrap_or(""),
                                    href
                                )
                            } else {
                                href.to_string()
                            }
                        } else {
                            href.to_string()
                        }
                    })
            })
        };

        Ok(LinkPreview {
            url: url.to_string(),
            title,
            description,
            image_url,
            site_name,
            favicon_url,
        })
    }
}

impl std::fmt::Debug for LinkProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkProcessor")
            .field("enabled", &self.config.enabled)
            .finish_non_exhaustive()
    }
}
