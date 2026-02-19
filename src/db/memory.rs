//! Memory repository for long-term memory storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

/// Column list for all memory SELECT queries
const MEMORY_COLUMNS: &str = "id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel, content_hash, origin_device_id, updated_at, deleted_at, synced_at, cloud_id";

/// Map a database row to a `MemoryRow`
fn row_to_memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
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
        content_hash: row.get(12)?,
        origin_device_id: row.get(13)?,
        updated_at: row.get(14)?,
        deleted_at: row.get(15)?,
        synced_at: row.get(16)?,
        cloud_id: row.get(17)?,
    })
}

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
    /// SHA-256 hash of content for dedup across devices
    pub content_hash: Option<String>,
    /// Device ID that originally created this memory
    pub origin_device_id: Option<String>,
    /// Last time this memory was modified
    pub updated_at: String,
    /// Soft-delete tombstone timestamp
    pub deleted_at: Option<String>,
    /// Last time this memory was synced to the cloud
    pub synced_at: Option<String>,
    /// API-side UUID for cross-device identity
    pub cloud_id: Option<String>,
}

impl Memory {
    /// Create a new memory item
    #[must_use]
    pub fn new(user_id: String, category: MemoryCategory, content: String) -> Self {
        let now = Utc::now();
        let content_hash = Self::compute_content_hash(&content);
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
            content_hash: Some(content_hash),
            origin_device_id: None,
            updated_at: now.to_rfc3339(),
            deleted_at: None,
            synced_at: None,
            cloud_id: None,
        }
    }

    /// Compute SHA-256 hash of content for dedup
    #[must_use]
    pub fn compute_content_hash(content: &str) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
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
            r"INSERT INTO memories (id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel, content_hash, origin_device_id, updated_at, deleted_at, synced_at, cloud_id)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                memory.content_hash,
                memory.origin_device_id,
                memory.updated_at,
                memory.deleted_at,
                memory.synced_at,
                memory.cloud_id,
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
            &format!("SELECT {} FROM memories WHERE id = ?1 AND deleted_at IS NULL", MEMORY_COLUMNS),
            [id],
            row_to_memory_row,
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

    /// Get a memory by ID without updating access stats (for sync operations)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_without_access_update(&self, id: &str) -> Result<Option<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            &format!("SELECT {} FROM memories WHERE id = ?1", MEMORY_COLUMNS),
            [id],
            row_to_memory_row,
        );

        match result {
            Ok(row) => Ok(Some(row.into_memory())),
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
                format!(
                    "SELECT {} FROM memories WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY pinned DESC, accessed_at DESC",
                    MEMORY_COLUMNS
                )
            },
            |cat| {
                format!(
                    "SELECT {} FROM memories WHERE user_id = ?1 AND category = '{}' AND deleted_at IS NULL ORDER BY pinned DESC, accessed_at DESC",
                    MEMORY_COLUMNS,
                    cat.as_str()
                )
            },
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([user_id], row_to_memory_row)?;

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

        let sql = format!(
            "SELECT {} FROM memories WHERE user_id = ?1 AND (content LIKE ?2 OR tags LIKE ?2) AND deleted_at IS NULL ORDER BY pinned DESC, accessed_at DESC",
            MEMORY_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let rows = stmt.query_map([user_id, &pattern], row_to_memory_row)?;

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

        let sql = format!(
            "SELECT {} FROM memories WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY pinned DESC, access_count DESC, accessed_at DESC LIMIT ?2",
            MEMORY_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        #[allow(clippy::cast_possible_wrap)]
        let limit = max_items as i64;
        let rows = stmt.query_map(rusqlite::params![user_id, limit], row_to_memory_row)?;

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
        let prefixed_columns = MEMORY_COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            r"SELECT {prefixed_columns}
              FROM memories m
              INNER JOIN (
                  SELECT memory_id, distance
                  FROM memories_vec
                  WHERE embedding MATCH ?1
                  ORDER BY distance
                  LIMIT ?2
              ) v ON m.id = v.memory_id
              WHERE m.user_id = ?3 AND m.deleted_at IS NULL
              ORDER BY v.distance"
        );
        let mut stmt = conn.prepare(&sql)?;

        #[allow(clippy::cast_possible_wrap)]
        let rows = stmt.query_map(rusqlite::params![embedding_bytes, limit as i64, user_id], row_to_memory_row)?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Hybrid search combining text substring match with vector similarity
    ///
    /// When an embedding is provided, results from both text search and
    /// vector search are merged and deduplicated. Text matches are
    /// prioritized (appear first), followed by vector-similar results.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn search_hybrid(
        &self,
        user_id: &str,
        query: &str,
        embedding: Option<&[f32]>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        // Start with text search results
        let mut results = self.search(user_id, query)?;
        let mut seen: std::collections::HashSet<String> =
            results.iter().map(|m| m.id.clone()).collect();

        // Merge in vector search results if embedding available
        if let Some(emb) = embedding {
            let similar = self.search_similar(user_id, emb, limit)?;
            for mem in similar {
                if seen.insert(mem.id.clone()) {
                    results.push(mem);
                }
            }
        }

        // Truncate to limit
        results.truncate(limit);
        Ok(results)
    }

    /// Soft-delete a memory (sets `deleted_at` tombstone)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        // Delete from vector table (no longer needed for search)
        conn.execute("DELETE FROM memories_vec WHERE memory_id = ?1", [id])?;

        // Soft-delete: set tombstone instead of removing the row
        let deleted = conn.execute(
            "UPDATE memories SET deleted_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1 AND deleted_at IS NULL",
            [id],
        )?;
        Ok(deleted > 0)
    }

    /// Hard-delete memories with tombstones older than the given cutoff
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn purge_tombstones(&self, cutoff_days: u32) -> Result<usize> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let deleted = conn.execute(
            "DELETE FROM memories WHERE deleted_at IS NOT NULL AND deleted_at < datetime('now', ?1)",
            [format!("-{cutoff_days} days")],
        )?;

        if deleted > 0 {
            tracing::info!(count = deleted, cutoff_days, "purged memory tombstones");
        }

        Ok(deleted)
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
            // Recompute content_hash when content changes
            updates.push("content_hash = ?");
            params.push(Box::new(Memory::compute_content_hash(c)));
        }
        if let Some(p) = pinned {
            updates.push("pinned = ?");
            params.push(Box::new(i32::from(p)));
        }

        if updates.is_empty() {
            return Ok(false);
        }

        // Always bump updated_at on modification
        updates.push("updated_at = datetime('now')");

        params.push(Box::new(id.to_string()));

        let sql = format!(
            "UPDATE memories SET {} WHERE id = ?",
            updates.join(", ")
        );

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(AsRef::as_ref).collect();
        let rows_affected = conn.execute(&sql, params_refs.as_slice())?;
        Ok(rows_affected > 0)
    }

    /// Query memories that need syncing (updated_at > synced_at or never synced)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn unsynced(&self) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM memories WHERE synced_at IS NULL OR updated_at > synced_at ORDER BY updated_at ASC",
            MEMORY_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_memory_row)?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
    }

    /// Mark memories as synced
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn mark_synced(&self, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE memories SET synced_at = datetime('now') WHERE id IN ({})",
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params.as_slice())?;

        Ok(())
    }

    /// Set the cloud_id for a memory after successful push
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn set_cloud_id(&self, id: &str, cloud_id: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let updated = conn.execute(
            "UPDATE memories SET cloud_id = ?1 WHERE id = ?2",
            rusqlite::params![cloud_id, id],
        )?;

        Ok(updated > 0)
    }

    /// Upsert a memory from a remote sync (used during pull)
    ///
    /// Uses content_hash for dedup: if a memory with the same user_id and
    /// content_hash exists, apply LWW merge. Otherwise insert as new.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn upsert_from_remote(&self, memory: &Memory) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let tags_json = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".to_string());

        // Check for existing memory by content_hash (dedup)
        if let Some(ref content_hash) = memory.content_hash {
            let existing: Option<(String, String, i32)> = conn
                .query_row(
                    "SELECT id, COALESCE(updated_at, created_at), access_count FROM memories WHERE user_id = ?1 AND content_hash = ?2",
                    rusqlite::params![memory.user_id, content_hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();

            if let Some((existing_id, existing_updated, existing_count)) = existing {
                // LWW merge: only update if remote is newer
                if memory.updated_at > existing_updated {
                    let merged_count = std::cmp::max(
                        memory.access_count,
                        u32::try_from(existing_count).unwrap_or(0),
                    );

                    conn.execute(
                        r"UPDATE memories SET
                            content = ?1, tags = ?2, pinned = ?3, access_count = ?4,
                            updated_at = ?5, deleted_at = ?6, cloud_id = ?7, synced_at = datetime('now')
                          WHERE id = ?8",
                        rusqlite::params![
                            memory.content,
                            tags_json,
                            i32::from(memory.pinned),
                            merged_count,
                            memory.updated_at,
                            memory.deleted_at,
                            memory.cloud_id,
                            existing_id,
                        ],
                    )?;

                    return Ok(true);
                }

                // Remote is older, only merge access_count
                let merged_count = std::cmp::max(
                    memory.access_count,
                    u32::try_from(existing_count).unwrap_or(0),
                );
                if merged_count > u32::try_from(existing_count).unwrap_or(0) {
                    conn.execute(
                        "UPDATE memories SET access_count = ?1, synced_at = datetime('now') WHERE id = ?2",
                        rusqlite::params![merged_count, existing_id],
                    )?;
                }

                return Ok(false);
            }
        }

        // No existing match, insert as new
        self.add(memory)?;

        // Mark as synced immediately since it came from the cloud
        conn.execute(
            "UPDATE memories SET synced_at = datetime('now') WHERE id = ?1",
            [&memory.id],
        )?;

        Ok(true)
    }

    /// Check if a memory with the given content hash already exists for a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn exists_by_content_hash(&self, user_id: &str, content_hash: &str) -> Result<bool> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND content_hash = ?2 AND deleted_at IS NULL",
            rusqlite::params![user_id, content_hash],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    /// Get exportable memories: pinned + top by access count
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_exportable(&self, user_id: &str, max_items: usize) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;

        let sql = format!(
            "SELECT {} FROM memories WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY pinned DESC, access_count DESC, accessed_at DESC LIMIT ?2",
            MEMORY_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        #[allow(clippy::cast_possible_wrap)]
        let limit = max_items as i64;
        let rows = stmt.query_map(rusqlite::params![user_id, limit], row_to_memory_row)?;

        let memories: Vec<Memory> = rows.flatten().map(MemoryRow::into_memory).collect();
        Ok(memories)
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
    content_hash: Option<String>,
    origin_device_id: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    synced_at: Option<String>,
    cloud_id: Option<String>,
}

impl MemoryRow {
    fn into_memory(self) -> Memory {
        let now_str = Utc::now().to_rfc3339();
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
            content_hash: self.content_hash,
            origin_device_id: self.origin_device_id,
            updated_at: self.updated_at.unwrap_or(now_str),
            deleted_at: self.deleted_at,
            synced_at: self.synced_at,
            cloud_id: self.cloud_id,
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
    fn test_search_hybrid_text_only() {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool);

        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("hybrid_user").unwrap();

        let m1 = Memory::new(user.id.clone(), MemoryCategory::Preference, "Likes dark mode".to_string());
        let m2 = Memory::new(user.id.clone(), MemoryCategory::Fact, "Works at Acme Corp".to_string());
        repo.add(&m1).unwrap();
        repo.add(&m2).unwrap();

        // Text-only hybrid search (no embedding)
        let results = repo.search_hybrid(&user.id, "dark", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Likes dark mode");
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
