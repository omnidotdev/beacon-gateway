//! Skill type definitions

use serde::{Deserialize, Serialize};

/// Skill metadata from SKILL.md frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// A loaded skill with content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub metadata: SkillMetadata,
    pub content: String,
    pub source: SkillSource,
}

/// Where the skill was loaded from
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Local filesystem
    Local,
    /// Manifold registry
    Manifold {
        namespace: String,
        repository: String,
    },
    /// Bundled with Beacon
    Bundled,
}

/// Skill installation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub skill: Skill,
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub enabled: bool,
}
