//! Knowledge pack repository for installed knowledge persistence

use uuid::Uuid;

use super::DbPool;
use crate::persona::KnowledgePack;
use crate::{Error, Result};

/// Column list for all knowledge pack SELECT queries
const PACK_COLUMNS: &str = "id, name, version, source_namespace, description, tags, chunk_count, has_embeddings, content, installed_at, updated_at";

/// Map a database row to a `KnowledgePackRow`
fn row_to_pack_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgePackRow> {
    Ok(KnowledgePackRow {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        source_namespace: row.get(3)?,
        description: row.get(4)?,
        tags: row.get(5)?,
        chunk_count: row.get(6)?,
        has_embeddings: row.get::<_, i32>(7)? != 0,
        content: row.get(8)?,
        installed_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

/// A knowledge pack row stored in the database
#[derive(Debug, Clone)]
pub struct KnowledgePackRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_namespace: String,
    pub description: Option<String>,
    /// JSON array of tags
    pub tags: String,
    pub chunk_count: i64,
    pub has_embeddings: bool,
    /// Full pack JSON
    pub content: String,
    pub installed_at: String,
    pub updated_at: String,
}

/// Knowledge pack repository for database operations
#[derive(Clone)]
pub struct KnowledgePackRepo {
    pool: DbPool,
}

impl KnowledgePackRepo {
    /// Create a new knowledge pack repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Install a knowledge pack
    ///
    /// Serializes the pack to JSON and stores it alongside metadata.
    /// If the pack includes pre-computed embeddings, they are inserted
    /// into the `knowledge_vec` virtual table.
    ///
    /// # Errors
    ///
    /// Returns error if database operation or serialization fails
    pub fn install(&self, pack: &KnowledgePack, namespace: &str) -> Result<String> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let id = format!("kp_{}", Uuid::new_v4());
        let tags_json = serde_json::to_string(&pack.tags)?;
        let content_json = serde_json::to_string(pack)?;

        #[allow(clippy::cast_possible_wrap)]
        let chunk_count = pack.chunks.len() as i64;
        let has_embeddings = pack.embeddings.is_some();

        conn.execute(
            &format!(
                "INSERT INTO installed_knowledge_packs ({PACK_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'), datetime('now'))"
            ),
            rusqlite::params![
                id,
                pack.name,
                pack.version,
                namespace,
                pack.description,
                tags_json,
                chunk_count,
                i32::from(has_embeddings),
                content_json,
            ],
        )?;

        // Insert embeddings into knowledge_vec if present
        if let Some(ref embeddings) = pack.embeddings {
            for (chunk_idx, embedding) in &embeddings.vectors {
                let chunk_id = format!("{id}:{chunk_idx}");
                let embedding_bytes = super::embedder::Embedder::to_bytes(embedding);

                conn.execute(
                    "INSERT INTO knowledge_vec (chunk_id, embedding) VALUES (?1, ?2)",
                    rusqlite::params![chunk_id, embedding_bytes],
                )?;
            }
        }

        tracing::info!(
            pack_id = %id,
            name = %pack.name,
            namespace = %namespace,
            chunk_count,
            has_embeddings,
            "knowledge pack installed"
        );

        Ok(id)
    }

    /// Uninstall a knowledge pack by name and namespace
    ///
    /// Removes the pack row and cleans up any associated vector entries.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn uninstall(&self, name: &str, namespace: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        // Look up the pack ID before deletion (needed for vector cleanup)
        let pack_id: Option<String> = conn
            .query_row(
                "SELECT id FROM installed_knowledge_packs WHERE name = ?1 AND source_namespace = ?2",
                rusqlite::params![name, namespace],
                |row| row.get(0),
            )
            .ok();

        let Some(pack_id) = pack_id else {
            return Ok(false);
        };

        // Clean up vector entries (chunk_id starts with pack_id)
        let prefix = format!("{pack_id}:%");
        conn.execute(
            "DELETE FROM knowledge_vec WHERE chunk_id LIKE ?1",
            rusqlite::params![prefix],
        )?;

        // Delete the pack row
        let deleted = conn.execute(
            "DELETE FROM installed_knowledge_packs WHERE id = ?1",
            rusqlite::params![pack_id],
        )?;

        if deleted > 0 {
            tracing::info!(
                pack_id = %pack_id,
                name = %name,
                namespace = %namespace,
                "knowledge pack uninstalled"
            );
        }

        Ok(deleted > 0)
    }

    /// Get a knowledge pack by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, id: &str) -> Result<Option<KnowledgePackRow>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            &format!("SELECT {PACK_COLUMNS} FROM installed_knowledge_packs WHERE id = ?1"),
            rusqlite::params![id],
            row_to_pack_row,
        );

        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get a knowledge pack by name and namespace
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_by_name(&self, name: &str, namespace: &str) -> Result<Option<KnowledgePackRow>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            &format!("SELECT {PACK_COLUMNS} FROM installed_knowledge_packs WHERE name = ?1 AND source_namespace = ?2"),
            rusqlite::params![name, namespace],
            row_to_pack_row,
        );

        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List all installed knowledge packs
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self) -> Result<Vec<KnowledgePackRow>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            &format!("SELECT {PACK_COLUMNS} FROM installed_knowledge_packs ORDER BY name"),
        )?;

        let rows = stmt.query_map([], row_to_pack_row)?;

        let mut packs = Vec::new();
        for row in rows {
            packs.push(row?);
        }

        Ok(packs)
    }

    /// Search knowledge chunk embeddings by vector similarity
    ///
    /// Returns `(chunk_id, distance)` pairs ordered by distance (closest first).
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn search_chunks(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let embedding_bytes = super::embedder::Embedder::to_bytes(query_embedding);

        let mut stmt = conn.prepare(
            r"SELECT chunk_id, distance
              FROM knowledge_vec
              WHERE embedding MATCH ?1
              ORDER BY distance
              LIMIT ?2",
        )?;

        #[allow(clippy::cast_possible_wrap)]
        let rows = stmt.query_map(
            rusqlite::params![embedding_bytes, limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;
    use crate::persona::{KnowledgeChunk, KnowledgePriority};

    fn test_pack() -> KnowledgePack {
        KnowledgePack {
            schema: None,
            version: "1.0.0".to_string(),
            name: "test-knowledge".to_string(),
            description: Some("A test knowledge pack".to_string()),
            tags: vec!["test".to_string(), "example".to_string()],
            chunks: vec![
                KnowledgeChunk {
                    topic: "Rust basics".to_string(),
                    tags: vec!["rust".to_string()],
                    content: "Rust is a systems programming language".to_string(),
                    rules: vec![],
                    priority: KnowledgePriority::Relevant,
                    embedding: None,
                },
                KnowledgeChunk {
                    topic: "Testing".to_string(),
                    tags: vec!["testing".to_string()],
                    content: "Write tests for your code".to_string(),
                    rules: vec![],
                    priority: KnowledgePriority::Always,
                    embedding: None,
                },
            ],
            embeddings: None,
        }
    }

    #[test]
    fn test_install_and_get() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").unwrap();
        assert!(id.starts_with("kp_"));

        let fetched = repo.get(&id).unwrap().unwrap();
        assert_eq!(fetched.name, "test-knowledge");
        assert_eq!(fetched.version, "1.0.0");
        assert_eq!(fetched.source_namespace, "omni");
        assert_eq!(fetched.chunk_count, 2);
        assert!(!fetched.has_embeddings);
    }

    #[test]
    fn test_get_by_name() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").unwrap();

        let fetched = repo.get_by_name("test-knowledge", "omni").unwrap().unwrap();
        assert_eq!(fetched.name, "test-knowledge");

        // Different namespace should not match
        let not_found = repo.get_by_name("test-knowledge", "other").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_list() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").unwrap();

        let packs = repo.list().unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "test-knowledge");
    }

    #[test]
    fn test_uninstall() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").unwrap();

        // Verify it exists
        assert!(repo.get(&id).unwrap().is_some());

        // Uninstall
        let removed = repo.uninstall("test-knowledge", "omni").unwrap();
        assert!(removed);

        // Verify it's gone
        assert!(repo.get(&id).unwrap().is_none());

        // Uninstalling again should return false
        let removed_again = repo.uninstall("test-knowledge", "omni").unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_unique_constraint() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").unwrap();

        // Installing the same name+namespace again should fail
        let result = repo.install(&pack, "omni");
        assert!(result.is_err());
    }

    #[test]
    fn test_content_roundtrip() {
        let pool = init_memory().unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").unwrap();

        let fetched = repo.get(&id).unwrap().unwrap();
        let deserialized: KnowledgePack = serde_json::from_str(&fetched.content).unwrap();
        assert_eq!(deserialized.name, "test-knowledge");
        assert_eq!(deserialized.chunks.len(), 2);
        assert_eq!(deserialized.chunks[0].topic, "Rust basics");
    }
}
