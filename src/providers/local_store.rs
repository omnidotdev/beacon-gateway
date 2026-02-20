//! Local SQLite storage for gateway-level provider keys (self-hosted deployments)

use crate::db::DbPool;
use crate::Result;

/// A stored provider key from the local database
#[derive(Debug, Clone)]
pub struct StoredKey {
    pub api_key: String,
    pub model_preference: Option<String>,
}

/// Gateway-local key store — used when Synapse is not configured
pub struct LocalKeyStore {
    db: DbPool,
}

impl LocalKeyStore {
    /// Create a new local key store backed by the given pool
    #[must_use]
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }

    /// Retrieve the stored key for a provider, or `None` if not configured
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn get(&self, provider: &str) -> Result<Option<StoredKey>> {
        let conn = self
            .db
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        let result = conn.query_row(
            "SELECT api_key, model_preference FROM local_provider_keys WHERE provider = ?1",
            rusqlite::params![provider],
            |row| {
                Ok(StoredKey {
                    api_key: row.get(0)?,
                    model_preference: row.get(1)?,
                })
            },
        );
        match result {
            Ok(key) => Ok(Some(key)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(crate::Error::Database(e.to_string())),
        }
    }

    /// Insert or replace the key for a provider
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    pub fn set(
        &self,
        provider: &str,
        api_key: &str,
        model_preference: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .db
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        conn.execute(
            "INSERT INTO local_provider_keys (provider, api_key, model_preference, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(provider) DO UPDATE SET
                api_key = excluded.api_key,
                model_preference = excluded.model_preference,
                updated_at = excluded.updated_at",
            rusqlite::params![provider, api_key, model_preference],
        )
        .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(())
    }

    /// Remove the stored key for a provider
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    pub fn remove(&self, provider: &str) -> Result<()> {
        let conn = self
            .db
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        conn.execute(
            "DELETE FROM local_provider_keys WHERE provider = ?1",
            rusqlite::params![provider],
        )
        .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(())
    }

    /// Return all provider names that have a locally stored key
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub fn list_configured(&self) -> Result<Vec<String>> {
        let conn = self
            .db
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT provider FROM local_provider_keys")
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        let providers: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| crate::Error::Database(e.to_string()))?
            .flatten()
            .collect();
        Ok(providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> crate::db::DbPool {
        crate::db::init_memory().unwrap()
    }

    #[test]
    fn set_and_get_key() {
        let store = LocalKeyStore::new(test_db());
        store.set("anthropic", "sk-ant-test", None).unwrap();
        let key = store.get("anthropic").unwrap();
        assert_eq!(key.unwrap().api_key, "sk-ant-test");
    }

    #[test]
    fn remove_key() {
        let store = LocalKeyStore::new(test_db());
        store.set("openai", "sk-openai-test", None).unwrap();
        store.remove("openai").unwrap();
        assert!(store.get("openai").unwrap().is_none());
    }

    #[test]
    fn list_configured_providers() {
        let store = LocalKeyStore::new(test_db());
        store.set("anthropic", "sk-ant-test", None).unwrap();
        store.set("openai", "sk-openai-test", None).unwrap();
        let configured = store.list_configured().unwrap();
        assert!(configured.contains(&"anthropic".to_string()));
        assert!(configured.contains(&"openai".to_string()));
        assert!(!configured.contains(&"openrouter".to_string()));
    }
}
