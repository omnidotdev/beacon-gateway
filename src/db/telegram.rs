//! Per-group Telegram configuration repository

use serde::{Deserialize, Serialize};

use super::DbPool;
use crate::{Error, Result};

/// Per-group Telegram configuration override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramGroupConfig {
    /// Telegram chat ID
    pub chat_id: String,
    /// Group title (informational, updated on upsert)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    /// Override for `require_mention_in_groups` (None = use global default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_mention: Option<bool>,
    /// Override for reaction level (None = use global default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_level: Option<String>,
    /// Override for ack reaction emoji
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack_reaction: Option<String>,
    /// Override for done reaction emoji
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_reaction: Option<String>,
    /// Whether the bot is enabled in this group
    pub enabled: bool,
}

/// Repository for per-group Telegram configuration
#[derive(Debug, Clone)]
pub struct TelegramGroupConfigRepo {
    pool: DbPool,
}

impl TelegramGroupConfigRepo {
    /// Create a new repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Get configuration for a specific chat
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, chat_id: &str) -> Result<Option<TelegramGroupConfig>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            "SELECT chat_id, chat_title, require_mention, reaction_level, ack_reaction, done_reaction, enabled
             FROM telegram_group_config WHERE chat_id = ?1",
            [chat_id],
            |row| {
                Ok(TelegramGroupConfig {
                    chat_id: row.get(0)?,
                    chat_title: row.get(1)?,
                    require_mention: row.get::<_, Option<i32>>(2)?.map(|v| v != 0),
                    reaction_level: row.get(3)?,
                    ack_reaction: row.get(4)?,
                    done_reaction: row.get(5)?,
                    enabled: row.get::<_, i32>(6)? != 0,
                })
            },
        );

        match result {
            Ok(config) => Ok(Some(config)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert configuration for a group
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn upsert(&self, config: &TelegramGroupConfig) -> Result<()> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        conn.execute(
            r"INSERT INTO telegram_group_config (chat_id, chat_title, require_mention, reaction_level, ack_reaction, done_reaction, enabled, updated_at)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
              ON CONFLICT(chat_id) DO UPDATE SET
                chat_title = excluded.chat_title,
                require_mention = excluded.require_mention,
                reaction_level = excluded.reaction_level,
                ack_reaction = excluded.ack_reaction,
                done_reaction = excluded.done_reaction,
                enabled = excluded.enabled,
                updated_at = datetime('now')",
            rusqlite::params![
                config.chat_id,
                config.chat_title,
                config.require_mention.map(|b| i32::from(b)),
                config.reaction_level,
                config.ack_reaction,
                config.done_reaction,
                i32::from(config.enabled),
            ],
        )?;

        Ok(())
    }

    /// Delete configuration for a group
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn delete(&self, chat_id: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let deleted = conn.execute(
            "DELETE FROM telegram_group_config WHERE chat_id = ?1",
            [chat_id],
        )?;

        Ok(deleted > 0)
    }

    /// List all group configurations
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self) -> Result<Vec<TelegramGroupConfig>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT chat_id, chat_title, require_mention, reaction_level, ack_reaction, done_reaction, enabled
             FROM telegram_group_config ORDER BY chat_title, chat_id",
        )?;

        let configs = stmt
            .query_map([], |row| {
                Ok(TelegramGroupConfig {
                    chat_id: row.get(0)?,
                    chat_title: row.get(1)?,
                    require_mention: row.get::<_, Option<i32>>(2)?.map(|v| v != 0),
                    reaction_level: row.get(3)?,
                    ack_reaction: row.get(4)?,
                    done_reaction: row.get(5)?,
                    enabled: row.get::<_, i32>(6)? != 0,
                })
            })?
            .flatten()
            .collect();

        Ok(configs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn telegram_group_config_crud() {
        let pool = db::init_memory().unwrap();
        let repo = TelegramGroupConfigRepo::new(pool);

        // Create
        let config = TelegramGroupConfig {
            chat_id: "-100123456".to_string(),
            chat_title: Some("Test Group".to_string()),
            require_mention: Some(true),
            reaction_level: Some("full".to_string()),
            ack_reaction: None,
            done_reaction: None,
            enabled: true,
        };
        repo.upsert(&config).unwrap();

        // Read
        let fetched = repo.get("-100123456").unwrap().unwrap();
        assert_eq!(fetched.chat_title.as_deref(), Some("Test Group"));
        assert_eq!(fetched.require_mention, Some(true));
        assert!(fetched.enabled);

        // Update
        let updated = TelegramGroupConfig {
            require_mention: Some(false),
            ..config.clone()
        };
        repo.upsert(&updated).unwrap();
        let fetched = repo.get("-100123456").unwrap().unwrap();
        assert_eq!(fetched.require_mention, Some(false));

        // List
        let all = repo.list().unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        assert!(repo.delete("-100123456").unwrap());
        assert!(repo.get("-100123456").unwrap().is_none());

        // Delete non-existent
        assert!(!repo.delete("-100123456").unwrap());
    }
}
