//! Web search tool
//!
//! Provides web search via configurable providers (Brave, Serper)

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Search provider configuration
#[derive(Debug, Clone)]
pub enum SearchProvider {
    /// Brave Search API
    Brave {
        /// API key for Brave Search
        api_key: String,
    },
    /// Serper (Google) Search API
    Serper {
        /// API key for Serper
        api_key: String,
    },
}

/// Web search tool
pub struct WebSearchTool {
    provider: SearchProvider,
    client: reqwest::Client,
}

/// Search result from web search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Result title
    pub title: String,
    /// Result URL
    pub url: String,
    /// Result snippet/description
    pub snippet: String,
}

/// Brave Search API response
#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: String,
}

/// Serper API response
#[derive(Debug, Deserialize)]
struct SerperSearchResponse {
    organic: Option<Vec<SerperResult>>,
}

#[derive(Debug, Deserialize)]
struct SerperResult {
    title: String,
    link: String,
    snippet: String,
}

/// Serper API request body
#[derive(Debug, Serialize)]
struct SerperRequest {
    q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    num: Option<usize>,
}

impl WebSearchTool {
    /// Create a new web search tool with Brave Search
    #[must_use]
    pub fn new_brave(api_key: String) -> Self {
        Self {
            provider: SearchProvider::Brave { api_key },
            client: reqwest::Client::new(),
        }
    }

    /// Create a new web search tool with Serper
    #[must_use]
    pub fn new_serper(api_key: String) -> Self {
        Self {
            provider: SearchProvider::Serper { api_key },
            client: reqwest::Client::new(),
        }
    }

    /// Perform a web search
    ///
    /// # Errors
    ///
    /// Returns error if the search request fails or response parsing fails
    pub async fn search(&self, query: &str, limit: Option<usize>) -> Result<Vec<SearchResult>> {
        match &self.provider {
            SearchProvider::Brave { api_key } => self.search_brave(api_key, query, limit).await,
            SearchProvider::Serper { api_key } => self.search_serper(api_key, query, limit).await,
        }
    }

    /// Search using Brave Search API
    async fn search_brave(
        &self,
        api_key: &str,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<SearchResult>> {
        let count = limit.unwrap_or(10);

        let response = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", api_key)
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await?;

        let response = response.error_for_status().map_err(Error::Http)?;

        let brave_response: BraveSearchResponse = response.json().await?;

        let results = brave_response
            .web
            .map(|web| {
                web.results
                    .into_iter()
                    .map(|r| SearchResult {
                        title: r.title,
                        url: r.url,
                        snippet: r.description,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Search using Serper API
    async fn search_serper(
        &self,
        api_key: &str,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<SearchResult>> {
        let request_body = SerperRequest {
            q: query.to_string(),
            num: limit,
        };

        let response = self
            .client
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        let response = response.error_for_status().map_err(Error::Http)?;

        let serper_response: SerperSearchResponse = response.json().await?;

        let results = serper_response
            .organic
            .map(|organic| {
                organic
                    .into_iter()
                    .map(|r| SearchResult {
                        title: r.title,
                        url: r.link,
                        snippet: r.snippet,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_brave() {
        let tool = WebSearchTool::new_brave("test-key".to_string());
        assert!(matches!(tool.provider, SearchProvider::Brave { .. }));
    }

    #[test]
    fn test_new_serper() {
        let tool = WebSearchTool::new_serper("test-key".to_string());
        assert!(matches!(tool.provider, SearchProvider::Serper { .. }));
    }
}
