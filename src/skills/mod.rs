//! Skills system for extensible agent capabilities

mod manifold;
mod types;

pub use manifold::ManifoldClient;
pub use types::{InstalledSkill, Skill, SkillMetadata, SkillSource};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Skill registry for discovery and management
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    cache_dir: PathBuf,
}

impl SkillRegistry {
    /// Create a new skill registry
    #[must_use]
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            skills: HashMap::new(),
            cache_dir,
        }
    }

    /// Discover skills from local directories
    ///
    /// # Errors
    ///
    /// Returns an error if a directory cannot be read
    pub fn discover_local(&mut self, dirs: &[PathBuf]) -> Result<usize> {
        let mut count = 0;
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            count += self.scan_directory(dir)?;
        }
        Ok(count)
    }

    /// Scan a directory for SKILL.md files
    fn scan_directory(&mut self, dir: &Path) -> Result<usize> {
        let mut count = 0;
        let entries = std::fs::read_dir(dir).map_err(|e| Error::Skill(e.to_string()))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }

            match load_skill_file(&skill_file) {
                Ok(skill) => {
                    self.skills.insert(skill.id.clone(), skill);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %skill_file.display(), error = %e, "failed to load skill");
                }
            }
        }
        Ok(count)
    }

    /// Get a skill by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.skills.get(id)
    }

    /// List all skills
    #[must_use]
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// Check if a skill exists
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.skills.contains_key(id)
    }

    /// Get the cache directory
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

/// Load a skill from a SKILL.md file
fn load_skill_file(path: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path).map_err(|e| Error::Skill(e.to_string()))?;

    let (metadata, body) = parse_frontmatter(&content)?;

    let id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(Skill {
        id,
        metadata,
        content: body,
        source: SkillSource::Local,
    })
}

/// Parse YAML frontmatter from markdown
fn parse_frontmatter(content: &str) -> Result<(SkillMetadata, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err(Error::Skill("missing frontmatter".to_string()));
    }

    let rest = &content[3..];
    let end = rest
        .find("---")
        .ok_or_else(|| Error::Skill("unclosed frontmatter".to_string()))?;

    let frontmatter = &rest[..end];
    let body = rest[end + 3..].trim().to_string();

    let metadata: SkillMetadata =
        serde_yaml::from_str(frontmatter).map_err(|e| Error::Skill(e.to_string()))?;

    Ok((metadata, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_works() {
        let content = r"---
name: test-skill
description: A test skill
tags:
  - testing
---

# Test Skill

This is the content.
";
        let (metadata, body) = parse_frontmatter(content).unwrap();
        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill");
        assert!(body.contains("# Test Skill"));
    }

    #[test]
    fn parse_frontmatter_missing_fails() {
        let content = "# No frontmatter";
        assert!(parse_frontmatter(content).is_err());
    }
}
