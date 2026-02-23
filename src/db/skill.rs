//! Skill repository for installed skills persistence

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DbPool;
use crate::skills::{InstalledSkill, Skill, SkillMetadata, SkillPriority, SkillSource};
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

    /// Install a skill with a specific priority and optional user scope
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn install_with_priority(
        &self,
        skill: &Skill,
        priority: SkillPriority,
        user_id: Option<&str>,
    ) -> Result<InstalledSkill> {
        // Generate command name before acquiring the INSERT connection to avoid
        // holding two pool connections simultaneously (deadlocks single-conn pools)
        let command_name = if skill.metadata.user_invocable {
            let existing = self.list_command_names()?;
            Some(crate::skills::deduplicate_command_name(
                &skill.metadata.name,
                &existing,
            ))
        } else {
            None
        };

        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(&skill.metadata.tags).unwrap_or_default();
        let permissions_json =
            serde_json::to_string(&skill.metadata.permissions).unwrap_or_default();
        let requires_env_json =
            serde_json::to_string(&skill.metadata.requires_env).unwrap_or_default();
        let os_json = serde_json::to_string(&skill.metadata.os).unwrap_or_default();
        let requires_bins_json =
            serde_json::to_string(&skill.metadata.requires_bins).unwrap_or_default();
        let requires_any_bins_json =
            serde_json::to_string(&skill.metadata.requires_any_bins).unwrap_or_default();

        let (source_type, source_namespace, source_repository) = match &skill.source {
            SkillSource::Local => ("local", None, None),
            SkillSource::Manifold {
                namespace,
                repository,
            } => ("manifold", Some(namespace.as_str()), Some(repository.as_str())),
            SkillSource::Bundled => ("bundled", None, None),
            SkillSource::Plugin => ("plugin", None, None),
        };

        // Compute command_dispatch_tool from metadata
        let command_dispatch_tool =
            if skill.metadata.command_dispatch.as_deref() == Some("tool") {
                skill.metadata.command_tool.clone()
            } else {
                None
            };

        let install_specs_json =
            serde_json::to_string(&skill.metadata.install).unwrap_or_else(|_| "[]".to_string());
        let requires_config_json =
            serde_json::to_string(&skill.metadata.requires_config).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            r"
            INSERT INTO installed_skills (
                id, name, description, version, author, tags, permissions,
                content, source_type, source_namespace, source_repository,
                enabled, priority, always_include, user_invocable,
                disable_model_invocation, emoji, requires_env, command_name, user_id,
                os, requires_bins, requires_any_bins, primary_env,
                command_dispatch_tool, api_key, skill_env,
                install_specs, requires_config
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23, ?24, NULL, '{}',
                ?25, ?26
            )
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
                priority.as_db(),
                skill.metadata.always,
                skill.metadata.user_invocable,
                skill.metadata.disable_model_invocation,
                skill.metadata.emoji,
                requires_env_json,
                command_name,
                user_id,
                os_json,
                requires_bins_json,
                requires_any_bins_json,
                skill.metadata.primary_env,
                command_dispatch_tool,
                install_specs_json,
                requires_config_json,
            ],
        )?;

        tracing::info!(skill_id = %id, name = %skill.metadata.name, priority = %priority.as_db(), "skill installed");

        let mut installed_skill = skill.clone();
        installed_skill.id = id;

        Ok(InstalledSkill {
            skill: installed_skill,
            installed_at: Utc::now(),
            enabled: true,
            priority,
            command_name,
            user_id: user_id.map(String::from),
            command_dispatch_tool,
            api_key: None,
            skill_env: HashMap::new(),
        })
    }

    /// Install a skill with default (standard) priority
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn install(&self, skill: &Skill) -> Result<InstalledSkill> {
        self.install_with_priority(skill, SkillPriority::default(), None)
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

    /// Column list for SELECT queries
    const SELECT_COLS: &str = r"
        id, name, description, version, author, tags, permissions,
        content, source_type, source_namespace, source_repository,
        enabled, installed_at, priority,
        always_include, user_invocable, disable_model_invocation,
        emoji, requires_env, command_name, user_id,
        os, requires_bins, requires_any_bins, primary_env,
        command_dispatch_tool, api_key, skill_env,
        install_specs, requires_config
    ";

    /// Get an installed skill by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, skill_id: &str) -> Result<Option<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM installed_skills WHERE id = ?1",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

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

        let sql = format!(
            "SELECT {} FROM installed_skills WHERE name = ?1",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

        let result = stmt.query_row(rusqlite::params![name], |row| {
            Self::row_to_installed_skill(row)
        });

        match result {
            Ok(skill) => Ok(Some(skill)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// Look up a skill by its slash command name, scoped to a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_by_command_name(
        &self,
        command: &str,
        user_id: Option<&str>,
    ) -> Result<Option<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM installed_skills WHERE command_name = ?1 AND (user_id IS NULL OR user_id = ?2)",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

        let result = stmt.query_row(
            rusqlite::params![command, user_id.unwrap_or("")],
            |row| Self::row_to_installed_skill(row),
        );

        match result {
            Ok(skill) => Ok(Some(skill)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// List all installed skills (admin view, no user filtering)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self) -> Result<Vec<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM installed_skills ORDER BY name",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map([], Self::row_to_installed_skill)?;

        let mut skills = Vec::new();
        for row in rows {
            skills.push(row?);
        }

        Ok(skills)
    }

    /// List enabled skills only (admin view, no user filtering)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_enabled(&self) -> Result<Vec<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM installed_skills WHERE enabled = 1 ORDER BY name",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map([], Self::row_to_installed_skill)?;

        let mut skills = Vec::new();
        for row in rows {
            skills.push(row?);
        }

        Ok(skills)
    }

    /// List enabled skills visible to a specific user (shared + user-specific)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_enabled_for_user(&self, user_id: Option<&str>) -> Result<Vec<InstalledSkill>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM installed_skills WHERE enabled = 1 AND (user_id IS NULL OR user_id = ?1) ORDER BY name",
            Self::SELECT_COLS,
        );
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map(
            rusqlite::params![user_id.unwrap_or("")],
            Self::row_to_installed_skill,
        )?;

        let mut skills = Vec::new();
        for row in rows {
            skills.push(row?);
        }

        Ok(skills)
    }

    /// List all existing command names (for deduplication)
    fn list_command_names(&self) -> Result<Vec<String>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT command_name FROM installed_skills WHERE command_name IS NOT NULL",
        )?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?;

        let mut names = Vec::new();
        for row in rows {
            names.push(row?);
        }
        Ok(names)
    }

    /// Upsert a bundled skill (INSERT OR REPLACE by name + bundled source)
    ///
    /// Preserves user settings (enabled, priority) when updating content.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn upsert_bundled(&self, skill: &Skill, priority: SkillPriority) -> Result<InstalledSkill> {
        // Check if already installed
        if let Some(existing) = self.get_by_name(&skill.metadata.name)? {
            if existing.skill.source == SkillSource::Bundled {
                // Update content but preserve user settings
                let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
                let requires_env_json =
                    serde_json::to_string(&skill.metadata.requires_env).unwrap_or_default();
                let os_json = serde_json::to_string(&skill.metadata.os).unwrap_or_default();
                let requires_bins_json =
                    serde_json::to_string(&skill.metadata.requires_bins).unwrap_or_default();
                let requires_any_bins_json =
                    serde_json::to_string(&skill.metadata.requires_any_bins).unwrap_or_default();
                let command_dispatch_tool =
                    if skill.metadata.command_dispatch.as_deref() == Some("tool") {
                        skill.metadata.command_tool.clone()
                    } else {
                        None
                    };
                let install_specs_json =
                    serde_json::to_string(&skill.metadata.install).unwrap_or_else(|_| "[]".to_string());
                let requires_config_json =
                    serde_json::to_string(&skill.metadata.requires_config).unwrap_or_else(|_| "[]".to_string());

                conn.execute(
                    r"
                    UPDATE installed_skills
                    SET content = ?1, description = ?2, always_include = ?3,
                        user_invocable = ?4, disable_model_invocation = ?5,
                        emoji = ?6, requires_env = ?7,
                        os = ?8, requires_bins = ?9, requires_any_bins = ?10,
                        primary_env = ?11, command_dispatch_tool = ?12,
                        install_specs = ?13, requires_config = ?14,
                        updated_at = datetime('now')
                    WHERE id = ?15
                    ",
                    rusqlite::params![
                        skill.content,
                        skill.metadata.description,
                        skill.metadata.always,
                        skill.metadata.user_invocable,
                        skill.metadata.disable_model_invocation,
                        skill.metadata.emoji,
                        requires_env_json,
                        os_json,
                        requires_bins_json,
                        requires_any_bins_json,
                        skill.metadata.primary_env,
                        command_dispatch_tool,
                        install_specs_json,
                        requires_config_json,
                        existing.skill.id,
                    ],
                )?;

                tracing::debug!(name = %skill.metadata.name, "updated bundled skill content");

                // Return with preserved settings
                return Ok(InstalledSkill {
                    skill: Skill {
                        id: existing.skill.id,
                        metadata: skill.metadata.clone(),
                        content: skill.content.clone(),
                        source: SkillSource::Bundled,
                    },
                    installed_at: existing.installed_at,
                    enabled: existing.enabled,
                    priority: existing.priority,
                    command_name: existing.command_name,
                    user_id: None,
                    command_dispatch_tool,
                    api_key: existing.api_key,
                    skill_env: existing.skill_env,
                });
            }
        }

        // Not found or not bundled — fresh install
        let mut bundled_skill = skill.clone();
        bundled_skill.source = SkillSource::Bundled;
        self.install_with_priority(&bundled_skill, priority, None)
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

    /// Set the priority of a skill
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_priority(&self, skill_id: &str, priority: SkillPriority) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let rows = conn.execute(
            r"
            UPDATE installed_skills
            SET priority = ?1, updated_at = datetime('now')
            WHERE id = ?2
            ",
            rusqlite::params![priority.as_db(), skill_id],
        )?;

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, priority = %priority.as_db(), "skill priority changed");
        }

        Ok(rows > 0)
    }

    /// Update skill configuration (api_key and/or env overrides)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn update_skill_config(
        &self,
        skill_id: &str,
        api_key: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut updates = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(key) = api_key {
            updates.push(format!("api_key = ?{idx}"));
            params.push(Box::new(key.to_string()));
            idx += 1;
        }
        if let Some(env_map) = env {
            let env_json = serde_json::to_string(env_map).unwrap_or_else(|_| "{}".to_string());
            updates.push(format!("skill_env = ?{idx}"));
            params.push(Box::new(env_json));
            idx += 1;
        }

        if updates.is_empty() {
            return Ok(false);
        }

        updates.push(format!("updated_at = datetime('now')"));
        let sql = format!(
            "UPDATE installed_skills SET {} WHERE id = ?{idx}",
            updates.join(", ")
        );
        params.push(Box::new(skill_id.to_string()));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = conn.execute(&sql, param_refs.as_slice())?;

        if rows > 0 {
            tracing::info!(skill_id = %skill_id, "skill config updated");
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
        let priority_str: String = row.get::<_, Option<String>>(13)?
            .unwrap_or_else(|| "standard".to_string());
        let always: bool = row.get::<_, Option<bool>>(14)?.unwrap_or(false);
        let user_invocable: bool = row.get::<_, Option<bool>>(15)?.unwrap_or(true);
        let disable_model_invocation: bool = row.get::<_, Option<bool>>(16)?.unwrap_or(false);
        let emoji: Option<String> = row.get(17)?;
        let requires_env_json: String = row.get::<_, Option<String>>(18)?
            .unwrap_or_else(|| "[]".to_string());
        let command_name: Option<String> = row.get(19)?;
        let user_id: Option<String> = row.get(20)?;

        // v14 columns (safe defaults for pre-migration DBs)
        let os_json: String = row.get::<_, Option<String>>(21)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_bins_json: String = row.get::<_, Option<String>>(22)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_any_bins_json: String = row.get::<_, Option<String>>(23)?
            .unwrap_or_else(|| "[]".to_string());
        let primary_env: Option<String> = row.get(24)?;
        let command_dispatch_tool: Option<String> = row.get(25)?;
        let api_key: Option<String> = row.get(26)?;
        let skill_env_json: String = row.get::<_, Option<String>>(27)?
            .unwrap_or_else(|| "{}".to_string());

        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let permissions: Vec<String> = serde_json::from_str(&permissions_json).unwrap_or_default();
        let requires_env: Vec<String> = serde_json::from_str(&requires_env_json).unwrap_or_default();
        let os: Vec<String> = serde_json::from_str(&os_json).unwrap_or_default();
        let requires_bins: Vec<String> = serde_json::from_str(&requires_bins_json).unwrap_or_default();
        let requires_any_bins: Vec<String> = serde_json::from_str(&requires_any_bins_json).unwrap_or_default();
        // v15/v16 columns
        let install_specs_json: String = row.get::<_, Option<String>>(28)?
            .unwrap_or_else(|| "[]".to_string());
        let requires_config_json: String = row.get::<_, Option<String>>(29)?
            .unwrap_or_else(|| "[]".to_string());

        let skill_env: HashMap<String, String> = serde_json::from_str(&skill_env_json).unwrap_or_default();
        let install: Vec<crate::skills::SkillInstallSpec> = serde_json::from_str(&install_specs_json).unwrap_or_default();
        let requires_config: Vec<String> = serde_json::from_str(&requires_config_json).unwrap_or_default();

        let source = match source_type.as_str() {
            "manifold" => SkillSource::Manifold {
                namespace: source_namespace.unwrap_or_default(),
                repository: source_repository.unwrap_or_default(),
            },
            "bundled" => SkillSource::Bundled,
            "plugin" => SkillSource::Plugin,
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
                    always,
                    user_invocable,
                    disable_model_invocation,
                    emoji,
                    requires_env,
                    os,
                    requires_bins,
                    requires_any_bins,
                    primary_env,
                    command_dispatch: None,
                    command_tool: None,
                    install,
                    requires_config,
                },
                content,
                source,
            },
            installed_at,
            enabled,
            priority: SkillPriority::from_db(&priority_str),
            command_name,
            user_id,
            command_dispatch_tool,
            api_key,
            skill_env,
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
                always: false,
                user_invocable: true,
                disable_model_invocation: false,
                emoji: None,
                requires_env: vec![],
                os: vec![],
                requires_bins: vec![],
                requires_any_bins: vec![],
                primary_env: None,
                command_dispatch: None,
                command_tool: None,
                install: vec![],
                requires_config: vec![],
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

    #[test]
    fn test_install_specs_db_roundtrip() {
        let pool = init_memory().unwrap();
        let repo = SkillRepo::new(pool);

        let mut skill = test_skill();
        skill.metadata.install = vec![crate::skills::SkillInstallSpec {
            kind: crate::skills::InstallKind::Brew,
            label: Some("jq".to_string()),
            bins: vec!["jq".to_string()],
            os: vec!["darwin".to_string(), "linux".to_string()],
            formula: Some("jq".to_string()),
            package: None,
            module: None,
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        }];
        skill.metadata.requires_config = vec!["voice.enabled".to_string()];

        let installed = repo.install(&skill).unwrap();
        let fetched = repo.get(&installed.skill.id).unwrap().unwrap();

        assert_eq!(fetched.skill.metadata.install.len(), 1);
        assert_eq!(fetched.skill.metadata.install[0].kind, crate::skills::InstallKind::Brew);
        assert_eq!(fetched.skill.metadata.install[0].formula.as_deref(), Some("jq"));
        assert_eq!(fetched.skill.metadata.requires_config, vec!["voice.enabled"]);
    }
}
