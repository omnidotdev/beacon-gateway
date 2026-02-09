//! Skill repository for installed skills persistence

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DbPool;
use crate::skills::{InstalledSkill, Skill, SkillMetadata, SkillSource};
use crate::{Error, Result};

/// Skill repository for CRUD operations on installed skills
#[derive(Clone)]
pub struct SkillRepo {
    pool: DbPool,
}

impl SkillRepo {
    /// Create a new skill repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Install a skill
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn install(&self, skill: &Skill) -> Result<InstalledSkill> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(&skill.metadata.tags).unwrap_or_default();
        let permissions_json =
            serde_json::to_string(&skill.metadata.permissions).unwrap_or_default();

        let (source_type, source_namespace, source_repository) = match &skill.source {
            SkillSource::Local => ("local", None, None),
            SkillSource::Manifold {
                namespace,
                repository,
            } => ("manifold", Some(namespace.as_str()), Some(repository.as_str())),
            SkillSource::Bundled => ("bundled", None, None),
        };

        conn.execute(
            r"
            INSERT INTO installed_skills (
                id, name, description, version, author, tags, permissions,
                content, source_type, source_namespace, source_repository, enabled
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1)
            ",
            rusqlite::params![
                id,
                skill.metadata.name,
                skill.metadata.description,
                skill.metadata.version,
                skill.metadata.author,
                tags_json,
                permissions_json,
                skill.content,
                source_type,
                source_namespace,
                source_repository,
            ],
        )?;

        tracing::info!(skill_id = %id, name = %skill.metadata.name, "skill installed");

        // Return with database ID
        let mut installed_skill = skill.clone();
        installed_skill.id = id;

        Ok(InstalledSkill {
            skill: installed_skill,
            installed_at: Utc::now(),
            enabled: true,
        })
    }

    /// Uninstall a skill by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn uninstall(&self, skill_id: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let rows = conn.execute(
            "DELETE FROM installed_skills WHERE id = ?1",
            rusqlite::params![skill_id],
        )?;

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, "skill uninstalled");
        }

        Ok(rows > 0)
    }

    /// Get an installed skill by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, skill_id: &str) -> Result<Option<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at
            FROM installed_skills
            WHERE id = ?1
            ",
        )?;

        let result = stmt.query_row(rusqlite::params![skill_id], |row| {
            Self::row_to_installed_skill(row)
        });

        match result {
            Ok(skill) => Ok(Some(skill)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// Get an installed skill by name
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_by_name(&self, name: &str) -> Result<Option<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at
            FROM installed_skills
            WHERE name = ?1
            ",
        )?;

        let result = stmt.query_row(rusqlite::params![name], |row| {
            Self::row_to_installed_skill(row)
        });

        match result {
            Ok(skill) => Ok(Some(skill)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// List all installed skills
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self) -> Result<Vec<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at
            FROM installed_skills
            ORDER BY name
            ",
        )?;

        let rows = stmt.query_map([], Self::row_to_installed_skill)?;

        let mut skills = Vec::new();
        for row in rows {
            skills.push(row?);
        }

        Ok(skills)
    }

    /// List enabled skills only
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_enabled(&self) -> Result<Vec<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at
            FROM installed_skills
            WHERE enabled = 1
            ORDER BY name
            ",
        )?;

        let rows = stmt.query_map([], Self::row_to_installed_skill)?;

        let mut skills = Vec::new();
        for row in rows {
            skills.push(row?);
        }

        Ok(skills)
    }

    /// Enable or disable a skill
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_enabled(&self, skill_id: &str, enabled: bool) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let rows = conn.execute(
            r"
            UPDATE installed_skills
            SET enabled = ?1, updated_at = datetime('now')
            WHERE id = ?2
            ",
            rusqlite::params![enabled, skill_id],
        )?;

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, enabled = %enabled, "skill enabled state changed");
        }

        Ok(rows > 0)
    }

    /// Convert a database row to an `InstalledSkill`
    fn row_to_installed_skill(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstalledSkill> {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let description: String = row.get(2)?;
        let version: Option<String> = row.get(3)?;
        let author: Option<String> = row.get(4)?;
        let tags_json: String = row.get(5)?;
        let permissions_json: String = row.get(6)?;
        let content: String = row.get(7)?;
        let source_type: String = row.get(8)?;
        let source_namespace: Option<String> = row.get(9)?;
        let source_repository: Option<String> = row.get(10)?;
        let enabled: bool = row.get(11)?;
        let installed_at: String = row.get(12)?;

        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let permissions: Vec<String> = serde_json::from_str(&permissions_json).unwrap_or_default();

        let source = match source_type.as_str() {
            "manifold" => SkillSource::Manifold {
                namespace: source_namespace.unwrap_or_default(),
                repository: source_repository.unwrap_or_default(),
            },
            "bundled" => SkillSource::Bundled,
            _ => SkillSource::Local,
        };

        let installed_at = DateTime::parse_from_rfc3339(&format!("{installed_at}Z"))
            .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

        Ok(InstalledSkill {
            skill: Skill {
                id,
                metadata: SkillMetadata {
                    name,
                    description,
                    version,
                    author,
                    tags,
                    permissions,
                },
                content,
                source,
            },
            installed_at,
            enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn test_skill() -> Skill {
        Skill {
            id: "test-skill".to_string(),
            metadata: SkillMetadata {
                name: "test-skill".to_string(),
                description: "A test skill".to_string(),
                version: Some("1.0.0".to_string()),
                author: Some("Test".to_string()),
                tags: vec!["test".to_string()],
                permissions: vec![],
            },
            content: "# Test Skill\n\nThis is a test.".to_string(),
            source: SkillSource::Local,
        }
    }

    #[test]
    fn test_install_and_get() {
        let pool = init_memory().unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).unwrap();

        assert_eq!(installed.skill.metadata.name, "test-skill");
        assert!(installed.enabled);

        let fetched = repo.get_by_name("test-skill").unwrap().unwrap();
        assert_eq!(fetched.skill.metadata.name, "test-skill");
    }

    #[test]
    fn test_list_and_uninstall() {
        let pool = init_memory().unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).unwrap();

        let list = repo.list().unwrap();
        assert_eq!(list.len(), 1);

        let removed = repo.uninstall(&installed.skill.id).unwrap();
        assert!(removed);

        let list = repo.list().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_enable_disable() {
        let pool = init_memory().unwrap();
        let repo = SkillRepo::new(pool);

        let skill = test_skill();
        let installed = repo.install(&skill).unwrap();

        // Disable
        repo.set_enabled(&installed.skill.id, false).unwrap();
        let fetched = repo.get(&installed.skill.id).unwrap().unwrap();
        assert!(!fetched.enabled);

        // List enabled should be empty
        let enabled = repo.list_enabled().unwrap();
        assert!(enabled.is_empty());

        // Re-enable
        repo.set_enabled(&installed.skill.id, true).unwrap();
        let enabled = repo.list_enabled().unwrap();
        assert_eq!(enabled.len(), 1);
    }
}
