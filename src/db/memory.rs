//! Memory repository for long-term memory storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

/// Memory categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    /// User preferences (how they like things done)
    Preference,
    /// Facts about the user or their environment
    Fact,
    /// Corrections from user feedback
    Correction,
    /// General learned information
    General,
}

impl MemoryCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Correction => "correction",
            Self::General => "general",
        }
    }

    fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "preference" => Some(Self::Preference),
            "fact" => Some(Self::Fact),
            "correction" => Some(Self::Correction),
            "general" => Some(Self::General),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A memory item stored in the database
#[derive(Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub user_id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub access_count: u32,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    /// Optional embedding for vector search
    pub embedding: Option<Vec<f32>>,
    /// Source session ID (where this memory was learned)
    pub source_session_id: Option<String>,
    /// Source channel (where this memory was learned)
    pub source_channel: Option<String>,
}

impl Memory {
    /// Create a new memory item
    #[must_use]
    pub fn new(user_id: String, category: MemoryCategory, content: String) -> Self {
        let now = Utc::now();
        Self {
            id: format!("mem_{}", Uuid::new_v4()),
            user_id,
            category,
            content,
            tags: Vec::new(),
            pinned: false,
            access_count: 0,
            created_at: now,
            accessed_at: now,
            embedding: None,
            source_session_id: None,
            source_channel: None,
        }
    }

    /// Add a tag to this memory
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Mark this memory as pinned
    #[must_use]
    pub const fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Set the embedding for this memory
    #[must_use]
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set the source session and channel
    #[must_use]
    pub fn with_source(mut self, session_id: String, channel: String) -> Self {
        self.source_session_id = Some(session_id);
        self.source_channel = Some(channel);
        self
    }
}

/// Memory repository for database operations
#[derive(Clone)]
pub struct MemoryRepo {
    pool: DbPool,
}

impl MemoryRepo {
    /// Create a new memory repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Add a new memory
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn add(&self, memory: &Memory) -> Result<()> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let tags_json = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".to_string());

        // Convert embedding to bytes if present
        let embedding_bytes = memory
            .embedding
            .as_ref()
            .map(|e| super::embedder::Embedder::to_bytes(e));

        conn.execute(
            r"INSERT INTO memories (id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                memory.id,
                memory.user_id,
                memory.category.as_str(),
                memory.content,
                tags_json,
                i32::from(memory.pinned),
                memory.access_count,
                memory.created_at.to_rfc3339(),
                memory.accessed_at.to_rfc3339(),
                embedding_bytes,
                memory.source_session_id,
                memory.source_channel,
            ],
        )?;

        // Also insert into vector table if embedding is present
        if let Some(ref embedding) = memory.embedding {
            let embedding_bytes = super::embedder::Embedder::to_bytes(embedding);
            conn.execute(
                "INSERT INTO memories_vec (memory_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![memory.id, embedding_bytes],
            )?;
        }

        Ok(())
    }

    /// Get a memory by ID (and update access stats)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, id: &str) -> Result<Option<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            r"SELECT id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel
              FROM memories WHERE id = ?1",
            [id],
            |row| {
                Ok(MemoryRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    category: row.get(2)?,
                    content: row.get(3)?,
                    tags: row.get(4)?,
                    pinned: row.get(5)?,
                    access_count: row.get(6)?,
                    created_at: row.get(7)?,
                    accessed_at: row.get(8)?,
                    embedding: row.get(9)?,
                    source_session_id: row.get(10)?,
                    source_channel: row.get(11)?,
                })
            },
        );

        match result {
            Ok(row) => {
                // Update access stats
                conn.execute(
                    r"UPDATE memories SET access_count = access_count + 1, accessed_at = datetime('now') WHERE id = ?1",
                    [id],
                )?;
                Ok(Some(row.into_memory()))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List memories for a user, optionally filtered by category
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self, user_id: &str, category: Option<MemoryCategory>) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = category.map_or_else(
            || {
                r"SELECT id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel
                  FROM memories WHERE user_id = ?1
                  ORDER BY pinned DESC, accessed_at DESC"
                    .to_string()
            },
            |cat| {
                format!(
                    r"SELECT id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel
                      FROM memories WHERE user_id = ?1 AND category = '{}'
                      ORDER BY pinned DESC, accessed_at DESC",
                    cat.as_str()
                )
            },
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([user_id], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                category: row.get(2)?,
                content: row.get(3)?,
                tags: row.get(4)?,
                pinned: row.get(5)?,
                access_count: row.get(6)?,
                created_at: row.get(7)?,
                accessed_at: row.get(8)?,
                embedding: row.get(9)?,
                source_session_id: row.get(10)?,
                source_channel: row.get(11)?,
            })
        })?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Search memories by content (simple substring match)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn search(&self, user_id: &str, query: &str) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let pattern = format!("%{query}%");

        let mut stmt = conn.prepare(
            r"SELECT id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel
              FROM memories WHERE user_id = ?1 AND (content LIKE ?2 OR tags LIKE ?2)
              ORDER BY pinned DESC, accessed_at DESC",
        )?;

        let rows = stmt.query_map([user_id, &pattern], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                category: row.get(2)?,
                content: row.get(3)?,
                tags: row.get(4)?,
                pinned: row.get(5)?,
                access_count: row.get(6)?,
                created_at: row.get(7)?,
                accessed_at: row.get(8)?,
                embedding: row.get(9)?,
                source_session_id: row.get(10)?,
                source_channel: row.get(11)?,
            })
        })?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Get memories for context injection (pinned + recent, up to `max_items`)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_context(&self, user_id: &str, max_items: usize) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn.prepare(
            r"SELECT id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel
              FROM memories WHERE user_id = ?1
              ORDER BY pinned DESC, access_count DESC, accessed_at DESC
              LIMIT ?2",
        )?;

        #[allow(clippy::cast_possible_wrap)]
        let limit = max_items as i64;
        let rows = stmt.query_map(rusqlite::params![user_id, limit], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                category: row.get(2)?,
                content: row.get(3)?,
                tags: row.get(4)?,
                pinned: row.get(5)?,
                access_count: row.get(6)?,
                created_at: row.get(7)?,
                accessed_at: row.get(8)?,
                embedding: row.get(9)?,
                source_session_id: row.get(10)?,
                source_channel: row.get(11)?,
            })
        })?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Search memories by vector similarity
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn search_similar(&self, user_id: &str, embedding: &[f32], limit: usize) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let embedding_bytes = super::embedder::Embedder::to_bytes(embedding);

        // Use sqlite-vec to find similar memories
        // Join with memories table to filter by user_id
        let mut stmt = conn.prepare(
            r"SELECT m.id, m.user_id, m.category, m.content, m.tags, m.pinned, m.access_count, m.created_at, m.accessed_at, m.embedding, m.source_session_id, m.source_channel
              FROM memories m
              INNER JOIN (
                  SELECT memory_id, distance
                  FROM memories_vec
                  WHERE embedding MATCH ?1
                  ORDER BY distance
                  LIMIT ?2
              ) v ON m.id = v.memory_id
              WHERE m.user_id = ?3
              ORDER BY v.distance",
        )?;

        #[allow(clippy::cast_possible_wrap)]
        let rows = stmt.query_map(rusqlite::params![embedding_bytes, limit as i64, user_id], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                category: row.get(2)?,
                content: row.get(3)?,
                tags: row.get(4)?,
                pinned: row.get(5)?,
                access_count: row.get(6)?,
                created_at: row.get(7)?,
                accessed_at: row.get(8)?,
                embedding: row.get(9)?,
                source_session_id: row.get(10)?,
                source_channel: row.get(11)?,
            })
        })?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Delete a memory
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        // Delete from vector table first (foreign key-like constraint)
        conn.execute("DELETE FROM memories_vec WHERE memory_id = ?1", [id])?;

        let deleted = conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
        Ok(deleted > 0)
    }

    /// Update a memory's embedding
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_embedding(&self, id: &str, embedding: &[f32]) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let embedding_bytes = super::embedder::Embedder::to_bytes(embedding);

        // Update in memories table
        let updated = conn.execute(
            "UPDATE memories SET embedding = ?1 WHERE id = ?2",
            rusqlite::params![embedding_bytes, id],
        )?;

        if updated == 0 {
            return Ok(false);
        }

        // Upsert in vector table
        conn.execute(
            "INSERT INTO memories_vec (memory_id, embedding) VALUES (?1, ?2)
             ON CONFLICT(memory_id) DO UPDATE SET embedding = excluded.embedding",
            rusqlite::params![id, embedding_bytes],
        )?;

        Ok(true)
    }

    /// Update a memory's content or pinned status
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn update(&self, id: &str, content: Option<&str>, pinned: Option<bool>) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let mut updates = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(c) = content {
            updates.push("content = ?");
            params.push(Box::new(c.to_string()));
        }
        if let Some(p) = pinned {
            updates.push("pinned = ?");
            params.push(Box::new(i32::from(p)));
        }

        if updates.is_empty() {
            return Ok(false);
        }

        params.push(Box::new(id.to_string()));

        let sql = format!(
            "UPDATE memories SET {} WHERE id = ?",
            updates.join(", ")
        );

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(AsRef::as_ref).collect();
        let rows_affected = conn.execute(&sql, params_refs.as_slice())?;
        Ok(rows_affected > 0)
    }

    /// Format memories for prompt injection
    #[must_use]
    pub fn format_for_prompt(memories: &[Memory]) -> String {
        use std::fmt::Write;

        if memories.is_empty() {
            return String::new();
        }

        let mut output = String::from("Remembered facts about you:\n");

        for mem in memories {
            let _ = writeln!(output, "- [{}] {}", mem.category, mem.content);
        }

        output
    }
}

/// Internal struct for database row mapping
struct MemoryRow {
    id: String,
    user_id: String,
    category: String,
    content: String,
    tags: String,
    pinned: i32,
    access_count: i32,
    created_at: String,
    accessed_at: String,
    embedding: Option<Vec<u8>>,
    source_session_id: Option<String>,
    source_channel: Option<String>,
}

impl MemoryRow {
    fn into_memory(self) -> Memory {
        Memory {
            id: self.id,
            user_id: self.user_id,
            category: MemoryCategory::from_str_value(&self.category).unwrap_or(MemoryCategory::General),
            content: self.content,
            tags: serde_json::from_str(&self.tags).unwrap_or_default(),
            pinned: self.pinned != 0,
            access_count: u32::try_from(self.access_count).unwrap_or(0),
            created_at: DateTime::parse_from_rfc3339(&self.created_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            accessed_at: DateTime::parse_from_rfc3339(&self.accessed_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            embedding: self
                .embedding
                .map(|b| super::embedder::Embedder::from_bytes(&b)),
            source_session_id: self.source_session_id,
            source_channel: self.source_channel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn test_memory_crud() {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool);

        // Create a user first
        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("test_user").unwrap();

        // Add a memory
        let memory = Memory::new(
            user.id.clone(),
            MemoryCategory::Preference,
            "User prefers dark mode".to_string(),
        );
        repo.add(&memory).unwrap();

        // Get the memory
        let fetched = repo.get(&memory.id).unwrap().unwrap();
        assert_eq!(fetched.content, "User prefers dark mode");
        assert_eq!(fetched.category, MemoryCategory::Preference);

        // List memories
        let memories = repo.list(&user.id, None).unwrap();
        assert_eq!(memories.len(), 1);

        // Search
        let found = repo.search(&user.id, "dark mode").unwrap();
        assert_eq!(found.len(), 1);

        // Delete
        assert!(repo.delete(&memory.id).unwrap());
        assert!(repo.get(&memory.id).unwrap().is_none());
    }

    #[test]
    fn test_memory_context() {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool);

        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("context_user").unwrap();

        // Add some memories
        let m1 = Memory::new(user.id.clone(), MemoryCategory::Preference, "Prefers vim".to_string());
        let m2 = Memory::new(user.id.clone(), MemoryCategory::Fact, "Lives in Seattle".to_string()).pinned();
        repo.add(&m1).unwrap();
        repo.add(&m2).unwrap();

        // Get context (pinned should come first)
        let context = repo.get_context(&user.id, 10).unwrap();
        assert_eq!(context.len(), 2);
        assert!(context[0].pinned); // Pinned first
    }

    #[test]
    fn test_format_for_prompt() {
        let memories = vec![
            Memory::new("u1".to_string(), MemoryCategory::Preference, "Uses vim".to_string()),
            Memory::new("u1".to_string(), MemoryCategory::Fact, "Works at Acme".to_string()),
        ];

        let formatted = MemoryRepo::format_for_prompt(&memories);
        assert!(formatted.contains("[preference] Uses vim"));
        assert!(formatted.contains("[fact] Works at Acme"));
    }
}
