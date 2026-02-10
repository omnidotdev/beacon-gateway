//! Manifold registry client for remote skills and personas
//!
//! Uses two Manifold API surfaces:
//! - Web router (`/@{namespace}/{repo}/{tag}`) for fetching individual artifacts
//! - OCI tag list (`/v2/{namespace}/{repo}/tags/list`) for listing artifacts

use reqwest::Client;
use serde::Deserialize;

use crate::{Error, Persona, Result};

use super::{Skill, SkillSource};

/// Manifold API client
#[derive(Debug, Clone)]
pub struct ManifoldClient {
    client: Client,
    base_url: String,
}

/// OCI tag list response
#[derive(Debug, Deserialize)]
struct TagListResponse {
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    tags: Vec<String>,
}

impl ManifoldClient {
    /// Create a new Manifold client
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch raw artifact content via the web router
    async fn fetch_artifact(&self, namespace: &str, repo: &str, tag: &str) -> Result<String> {
        let url = format!("{}/@{}/{}/{}", self.base_url, namespace, repo, tag);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Manifold(format!(
                "artifact not found: @{}/{}/{} ({})",
                namespace,
                repo,
                tag,
                response.status()
            )));
        }

        response
            .text()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))
    }

    /// List tags in a repository via OCI tag list API
    async fn list_tags(&self, namespace: &str, repo: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/v2/{}/{}/tags/list",
            self.base_url, namespace, repo
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(Error::Manifold(format!(
                "failed to list tags: {}",
                response.status()
            )));
        }

        let tag_list: TagListResponse = response
            .json()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        Ok(tag_list.tags)
    }

    /// List skills from a namespace
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response cannot be parsed
    pub async fn list_skills(&self, namespace: &str) -> Result<Vec<Skill>> {
        let tags = self.list_tags(namespace, "skills").await?;

        let mut skills = Vec::new();
        for tag in &tags {
            match self.fetch_artifact(namespace, "skills", tag).await {
                Ok(content) => {
                    if let Ok(skill) = Self::parse_skill_content(&content, namespace) {
                        skills.push(skill);
                    }
                }
                Err(e) => {
                    tracing::debug!(tag, error = %e, "skipping skill");
                }
            }
        }

        Ok(skills)
    }

    /// Fetch a specific skill
    ///
    /// # Errors
    ///
    /// Returns an error if the skill is not found or cannot be parsed
    pub async fn get_skill(&self, namespace: &str, skill_id: &str) -> Result<Skill> {
        let content = self.fetch_artifact(namespace, "skills", skill_id).await?;
        Self::parse_skill_content(&content, namespace)
    }

    /// Parse skill content from Manifold artifact
    fn parse_skill_content(content: &str, namespace: &str) -> Result<Skill> {
        let (metadata, body) = super::parse_frontmatter(content)?;

        Ok(Skill {
            id: metadata.name.clone(),
            metadata,
            content: body,
            source: SkillSource::Manifold {
                namespace: namespace.to_string(),
                repository: "skills".to_string(),
            },
        })
    }

    /// Search skills by query
    ///
    /// # Errors
    ///
    /// Returns an error if the search fails
    pub async fn search_skills(&self, query: &str) -> Result<Vec<Skill>> {
        // TODO: implement cross-namespace search when Manifold supports it
        let skills = self.list_skills("community").await?;

        let query_lower = query.to_lowercase();
        let filtered: Vec<Skill> = skills
            .into_iter()
            .filter(|s| {
                s.metadata.name.to_lowercase().contains(&query_lower)
                    || s.metadata
                        .description
                        .to_lowercase()
                        .contains(&query_lower)
                    || s.metadata
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .collect();

        Ok(filtered)
    }

    // =========================================================================
    // Persona methods
    // =========================================================================

    /// List personas from a namespace
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response cannot be parsed
    pub async fn list_personas(&self, namespace: &str) -> Result<Vec<Persona>> {
        let tags = self.list_tags(namespace, "personas").await?;

        let mut personas = Vec::new();
        for tag in &tags {
            match self.fetch_artifact(namespace, "personas", tag).await {
                Ok(content) => {
                    if let Ok(persona) = serde_json::from_str::<Persona>(&content) {
                        personas.push(persona);
                    }
                }
                Err(e) => {
                    tracing::debug!(tag, error = %e, "skipping persona");
                }
            }
        }

        Ok(personas)
    }

    /// Fetch a specific persona
    ///
    /// # Errors
    ///
    /// Returns an error if the persona is not found or cannot be parsed
    pub async fn get_persona(&self, namespace: &str, persona_id: &str) -> Result<Persona> {
        let content = self
            .fetch_artifact(namespace, "personas", persona_id)
            .await?;

        serde_json::from_str(&content).map_err(|e| Error::Manifold(e.to_string()))
    }

    /// Search personas by query
    ///
    /// # Errors
    ///
    /// Returns an error if the search fails
    pub async fn search_personas(&self, query: &str) -> Result<Vec<Persona>> {
        let personas = self.list_personas("community").await?;

        if query.is_empty() {
            return Ok(personas);
        }

        let query_lower = query.to_lowercase();
        let filtered: Vec<Persona> = personas
            .into_iter()
            .filter(|p| {
                p.identity.name.to_lowercase().contains(&query_lower)
                    || p.identity
                        .tagline
                        .as_ref()
                        .is_some_and(|t| t.to_lowercase().contains(&query_lower))
                    || p.identity
                        .description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&query_lower))
            })
            .collect();

        Ok(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_creation() {
        let client = ManifoldClient::new("https://api.manifold.omni.dev");
        assert_eq!(client.base_url, "https://api.manifold.omni.dev");
    }

    #[test]
    fn client_trims_trailing_slash() {
        let client = ManifoldClient::new("https://api.manifold.omni.dev/");
        assert_eq!(client.base_url, "https://api.manifold.omni.dev");
    }
}
