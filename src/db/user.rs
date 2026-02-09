//! User repository for CRUD operations

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

/// A user
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub life_json_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User context entry (learned preference)
#[derive(Debug, Clone)]
pub struct UserContext {
    pub id: String,
    pub user_id: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User repository
#[derive(Clone)]
pub struct UserRepo {
    pool: DbPool,
}

impl UserRepo {
    /// Create a new user repository
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Find or create a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn find_or_create(&self, id: &str) -> Result<User> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        // Try to find existing user
        let existing: Option<User> = conn
            .query_row(
                "SELECT id, life_json_path, created_at, updated_at FROM users WHERE id = ?1",
                [id],
                |row| {
                    Ok(User {
                        id: row.get(0)?,
                        life_json_path: row.get(1)?,
                        created_at: parse_datetime(&row.get::<_, String>(2)?),
                        updated_at: parse_datetime(&row.get::<_, String>(3)?),
                    })
                },
            )
            .ok();

        if let Some(user) = existing {
            return Ok(user);
        }

        // Create new user
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO users (id, created_at, updated_at) VALUES (?1, ?2, ?2)",
            [id, &now],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(User {
            id: id.to_string(),
            life_json_path: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// Find a user by ID (returns None if not found)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn find(&self, id: &str) -> Result<Option<User>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let user = conn
            .query_row(
                "SELECT id, life_json_path, created_at, updated_at FROM users WHERE id = ?1",
                [id],
                |row| {
                    Ok(User {
                        id: row.get(0)?,
                        life_json_path: row.get(1)?,
                        created_at: parse_datetime(&row.get::<_, String>(2)?),
                        updated_at: parse_datetime(&row.get::<_, String>(3)?),
                    })
                },
            )
            .ok();

        Ok(user)
    }

    /// List all users
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_all(&self) -> Result<Vec<User>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare("SELECT id, life_json_path, created_at, updated_at FROM users ORDER BY created_at DESC")
            .map_err(|e| Error::Database(e.to_string()))?;

        let users = stmt
            .query_map([], |row| {
                Ok(User {
                    id: row.get(0)?,
                    life_json_path: row.get(1)?,
                    created_at: parse_datetime(&row.get::<_, String>(2)?),
                    updated_at: parse_datetime(&row.get::<_, String>(3)?),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(users)
    }

    /// Delete a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn delete(&self, id: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        conn.execute("DELETE FROM users WHERE id = ?1", [id])
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }

    /// Set user's life.json path
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_life_json_path(&self, user_id: &str, path: Option<&str>) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE users SET life_json_path = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![path, now, user_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }

    /// Get user context (learned preferences)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_context(&self, user_id: &str) -> Result<Vec<UserContext>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, key, value, source, created_at, updated_at
                 FROM user_context WHERE user_id = ?1 ORDER BY key",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let contexts = stmt
            .query_map([user_id], |row| {
                Ok(UserContext {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    source: row.get(4)?,
                    created_at: parse_datetime(&row.get::<_, String>(5)?),
                    updated_at: parse_datetime(&row.get::<_, String>(6)?),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(contexts)
    }

    /// Set a user context value
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_context(&self, user_id: &str, key: &str, value: &str, source: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO user_context (id, user_id, key, value, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(user_id, key) DO UPDATE SET value = ?4, source = ?5, updated_at = ?6",
            [&id, user_id, key, value, source, &now],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }

    /// Get a specific context value
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_context_value(&self, user_id: &str, key: &str) -> Result<Option<String>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM user_context WHERE user_id = ?1 AND key = ?2",
                [user_id, key],
                |row| row.get(0),
            )
            .ok();

        Ok(value)
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup() -> UserRepo {
        let pool = init_memory().unwrap();
        UserRepo::new(pool)
    }

    #[test]
    fn test_find_or_create_user() {
        let repo = setup();

        let user = repo.find_or_create("user-123").unwrap();
        assert_eq!(user.id, "user-123");
        assert!(user.life_json_path.is_none());

        // Should return same user
        let user2 = repo.find_or_create("user-123").unwrap();
        assert_eq!(user.id, user2.id);
    }

    #[test]
    fn test_set_life_json_path() {
        let repo = setup();

        let user = repo.find_or_create("user-456").unwrap();
        repo.set_life_json_path(&user.id, Some("/home/user/life.json"))
            .unwrap();

        let user = repo.find_or_create("user-456").unwrap();
        assert_eq!(user.life_json_path.as_deref(), Some("/home/user/life.json"));
    }

    #[test]
    fn test_user_context() {
        let repo = setup();

        let user = repo.find_or_create("user-789").unwrap();

        // Set context
        repo.set_context(&user.id, "timezone", "America/Los_Angeles", "learned")
            .unwrap();
        repo.set_context(&user.id, "name", "Brian", "life.json")
            .unwrap();

        // Get all context
        let contexts = repo.get_context(&user.id).unwrap();
        assert_eq!(contexts.len(), 2);

        // Get specific value
        let tz = repo.get_context_value(&user.id, "timezone").unwrap();
        assert_eq!(tz, Some("America/Los_Angeles".to_string()));

        // Update existing context
        repo.set_context(&user.id, "timezone", "America/New_York", "learned")
            .unwrap();
        let tz = repo.get_context_value(&user.id, "timezone").unwrap();
        assert_eq!(tz, Some("America/New_York".to_string()));
    }
}
