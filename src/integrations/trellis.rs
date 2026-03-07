//! Trellis knowledge garden integration
//!
//! Provides bidirectional sync between Beacon memories and Trellis notes:
//! - Memories can be promoted to Trellis notes
//! - Trellis notes can feed into agent context

use serde::{Deserialize, Serialize};

/// Client for the Trellis knowledge garden API
pub struct TrellisClient {
    base_url: String,
    client: reqwest::Client,
    api_key: Option<String>,
}

/// A note in the Trellis knowledge garden
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrellisNote {
    /// Note ID (assigned by Trellis)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Note title
    pub title: String,
    /// Markdown content
    pub content: String,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
    /// Source attribution (e.g. "beacon:memory:abc123")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Search results from Trellis
#[derive(Debug, Deserialize)]
pub struct TrellisSearchResult {
    /// Matching notes
    pub notes: Vec<TrellisNote>,
    /// Total matches (may exceed returned count)
    pub total: usize,
}

impl TrellisClient {
    /// Create a new Trellis client
    #[must_use]
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            api_key,
        }
    }

    /// Check if the Trellis service is reachable
    ///
    /// # Errors
    ///
    /// Returns error if the health check fails
    pub async fn health_check(&self) -> Result<bool, String> {
        let url = format!("{}/health", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(e) => Err(format!("Trellis health check failed: {e}")),
        }
    }

    /// Save a note to the knowledge garden
    ///
    /// # Errors
    ///
    /// Returns error if the request fails
    pub async fn save_note(&self, note: &TrellisNote) -> Result<TrellisNote, String> {
        let url = format!("{}/api/notes", self.base_url);
        let mut req = self.client.post(&url).json(note);

        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Trellis save failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Trellis save returned {status}: {body}"));
        }

        resp.json()
            .await
            .map_err(|e| format!("Trellis response parse error: {e}"))
    }

    /// Search the knowledge garden
    ///
    /// # Errors
    ///
    /// Returns error if the request fails
    pub async fn search(&self, query: &str, limit: usize) -> Result<TrellisSearchResult, String> {
        let url = format!("{}/api/notes/search", self.base_url);
        let mut req = self
            .client
            .get(&url)
            .query(&[("q", query), ("limit", &limit.to_string())]);

        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Trellis search failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Trellis search returned {status}: {body}"));
        }

        resp.json()
            .await
            .map_err(|e| format!("Trellis search parse error: {e}"))
    }

    /// Get a note by ID
    ///
    /// # Errors
    ///
    /// Returns error if the request fails
    pub async fn get_note(&self, id: &str) -> Result<Option<TrellisNote>, String> {
        let url = format!("{}/api/notes/{id}", self.base_url);
        let mut req = self.client.get(&url);

        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Trellis get note failed: {e}"))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Trellis get note returned {status}: {body}"));
        }

        resp.json()
            .await
            .map(Some)
            .map_err(|e| format!("Trellis note parse error: {e}"))
    }

    /// Delete a note by ID
    ///
    /// # Errors
    ///
    /// Returns error if the request fails
    pub async fn delete_note(&self, id: &str) -> Result<(), String> {
        let url = format!("{}/api/notes/{id}", self.base_url);
        let mut req = self.client.delete(&url);

        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Trellis delete failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Trellis delete returned {status}: {body}"));
        }

        Ok(())
    }
}
