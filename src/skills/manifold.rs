//! Manifold registry client for remote skills and personas

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

/// Artifact response from Manifold API
#[derive(Debug, Deserialize)]
struct ArtifactResponse {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    digest: String,
    #[allow(dead_code)]
    media_type: String,
    #[serde(default)]
    content: Option<String>,
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

    /// List skills from a namespace
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response cannot be parsed
    pub async fn list_skills(&self, namespace: &str) -> Result<Vec<Skill>> {
        let url = format!(
            "{}/api/namespaces/{}/repositories/skills/artifacts",
            self.base_url, namespace
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Manifold(format!(
                "failed to list skills: {}",
                response.status()
            )));
        }

        let artifacts: Vec<ArtifactResponse> = response
            .json()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        let mut skills = Vec::new();
        for artifact in artifacts {
            if let Some(content) = artifact.content {
                if let Ok(skill) = Self::parse_skill_content(&content, namespace) {
                    skills.push(skill);
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
        let url = format!(
            "{}/api/namespaces/{}/repositories/skills/artifacts/{}",
            self.base_url, namespace, skill_id
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Manifold(format!(
                "skill not found: {}",
                response.status()
            )));
        }

        let artifact: ArtifactResponse = response
            .json()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        let content = artifact
            .content
            .ok_or_else(|| Error::Manifold("skill has no content".to_string()))?;

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
        // For now, search within the "community" namespace
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
        let url = format!(
            "{}/api/namespaces/{}/repositories/personas/artifacts",
            self.base_url, namespace
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            // Return empty list if personas repo doesn't exist yet
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(Error::Manifold(format!(
                "failed to list personas: {}",
                response.status()
            )));
        }

        let artifacts: Vec<ArtifactResponse> = response
            .json()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        let mut personas = Vec::new();
        for artifact in artifacts {
            if let Some(content) = artifact.content {
                if let Ok(persona) = serde_json::from_str::<Persona>(&content) {
                    personas.push(persona);
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
        let url = format!(
            "{}/api/namespaces/{}/repositories/personas/artifacts/{}",
            self.base_url, namespace, persona_id
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::Manifold(format!(
                "persona not found: {}",
                response.status()
            )));
        }

        let artifact: ArtifactResponse = response
            .json()
            .await
            .map_err(|e| Error::Manifold(e.to_string()))?;

        let content = artifact
            .content
            .ok_or_else(|| Error::Manifold("persona has no content".to_string()))?;

        serde_json::from_str(&content).map_err(|e| Error::Manifold(e.to_string()))
    }

    /// Search personas by query
    ///
    /// # Errors
    ///
    /// Returns an error if the search fails
    pub async fn search_personas(&self, query: &str) -> Result<Vec<Persona>> {
        // Search within the "community" namespace
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
        let client = ManifoldClient::new("https://manifold.omni.dev");
        assert_eq!(client.base_url, "https://manifold.omni.dev");
    }

    #[test]
    fn client_trims_trailing_slash() {
        let client = ManifoldClient::new("https://manifold.omni.dev/");
        assert_eq!(client.base_url, "https://manifold.omni.dev");
    }
}
