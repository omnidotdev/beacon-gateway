//! Memory repository for long-term memory storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

/// Half-life for temporal decay (in days)
const DECAY_HALF_LIFE_DAYS: f64 = 7.0;

/// Weight of temporal decay in combined scoring (0.0 = no decay effect, 1.0 = full effect)
const DECAY_WEIGHT: f64 = 0.3;

/// Compute temporal decay factor for a memory based on its `accessed_at` timestamp.
///
/// Uses exponential decay with a configurable half-life.
/// Returns a value in `[0.0, 1.0]` where 1.0 means just accessed and ~0.0 means very old.
#[must_use]
pub fn temporal_decay_factor(accessed_at: &DateTime<Utc>, now: &DateTime<Utc>) -> f64 {
    let elapsed_days = (*now - *accessed_at).num_seconds().max(0) as f64 / 86400.0;
    // Exponential decay: 2^(-t/half_life)
    (-elapsed_days / DECAY_HALF_LIFE_DAYS).exp2()
}

/// Compute cosine similarity between two vectors.
///
/// Returns a value in `[-1.0, 1.0]` where 1.0 is identical direction.
/// Returns 0.0 if either vector has zero magnitude.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;

    for (ai, bi) in a.iter().zip(b.iter()) {
        let ai = f64::from(*ai);
        let bi = f64::from(*bi);
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f64::EPSILON {
        return 0.0;
    }

    (dot / denom) as f32
}

/// Candidate for MMR re-ranking
struct MmrCandidate {
    memory: Memory,
    score: f64,
}

/// Apply Maximal Marginal Relevance re-ranking to a list of scored candidates.
///
/// Iteratively selects candidates that balance relevance (score) with diversity
/// (dissimilarity to already-selected items). Lambda controls the trade-off:
/// higher lambda favors relevance, lower lambda favors diversity.
fn mmr_rerank(candidates: Vec<MmrCandidate>, limit: usize, lambda: f64) -> Vec<Memory> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut remaining: Vec<MmrCandidate> = candidates;
    let mut selected: Vec<(Memory, Option<Vec<f32>>)> = Vec::with_capacity(limit);

    // Normalize scores to [0, 1] for MMR calculation
    let max_score = remaining
        .iter()
        .map(|c| c.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_score = remaining
        .iter()
        .map(|c| c.score)
        .fold(f64::INFINITY, f64::min);
    let score_range = max_score - min_score;

    while selected.len() < limit && !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_mmr = f64::NEG_INFINITY;

        for (i, candidate) in remaining.iter().enumerate() {
            let relevance = if score_range > f64::EPSILON {
                (candidate.score - min_score) / score_range
            } else {
                1.0
            };

            // Max similarity to any already-selected item
            let max_sim = if selected.is_empty() {
                0.0
            } else {
                candidate
                    .memory
                    .embedding
                    .as_ref()
                    .map(|emb| {
                        selected
                            .iter()
                            .filter_map(|(_, sel_emb)| {
                                sel_emb.as_ref().map(|se| f64::from(cosine_similarity(emb, se)))
                            })
                            .fold(0.0_f64, f64::max)
                    })
                    .unwrap_or(0.0)
            };

            let mmr_score = lambda * relevance - (1.0 - lambda) * max_sim;
            if mmr_score > best_mmr {
                best_mmr = mmr_score;
                best_idx = i;
            }
        }

        let chosen = remaining.swap_remove(best_idx);
        let emb = chosen.memory.embedding.clone();
        selected.push((chosen.memory, emb));
    }

    selected.into_iter().map(|(m, _)| m).collect()
}

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

    pub fn from_str_value(s: &str) -> Option<Self> {
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
#[derive(Debug, Clone)]
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

    /// Search memories by vector similarity with temporal decay and MMR diversity re-ranking.
    ///
    /// Over-fetches 3x candidates, applies temporal decay to distance scores,
    /// then uses MMR to select diverse results before returning the top `limit`.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn search_similar(&self, user_id: &str, embedding: &[f32], limit: usize) -> Result<Vec<Memory>> {
        let conn = self.pool.get().map_err(|e| Error::Database(e.to_string()))?;
        let embedding_bytes = super::embedder::Embedder::to_bytes(embedding);

        // Over-fetch 3x candidates for temporal decay + MMR re-ranking
        let fetch_limit = limit * 3;

        // Use sqlite-vec to find similar memories
        // Join with memories table to filter by user_id
        let prefixed_columns = MEMORY_COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            r"SELECT {prefixed_columns}, v.distance
              FROM memories m
              INNER JOIN (
                  SELECT memory_id, distance
                  FROM memories_vec
                  WHERE embedding MATCH ?1
                  ORDER BY distance
                  LIMIT ?2
              ) v ON m.id = v.memory_id
              WHERE m.user_id = ?3 AND m.deleted_at IS NULL"
        );
        let mut stmt = conn.prepare(&sql)?;

        #[allow(clippy::cast_possible_wrap)]
        let rows = stmt.query_map(
            rusqlite::params![embedding_bytes, fetch_limit as i64, user_id],
            |row| {
                let memory_row = row_to_memory_row(row)?;
                let distance: f64 = row.get(18)?; // distance is the 19th column (0-indexed)
                Ok((memory_row, distance))
            },
        )?;

        let now = Utc::now();
        let candidates: Vec<MmrCandidate> = rows
            .flatten()
            .map(|(row, distance)| {
                let memory = row.into_memory();
                let decay = temporal_decay_factor(&memory.accessed_at, &now);
                // Lower distance = better. Apply temporal decay as a bonus for recent memories.
                // Combined score: lower is better (subtract decay bonus)
                let combined = distance * (1.0 - DECAY_WEIGHT * decay);
                // Invert so higher = better for MMR (MMR selects highest scores)
                let score = 1.0 / (1.0 + combined);
                MmrCandidate { memory, score }
            })
            .collect();

        Ok(mmr_rerank(candidates, limit, 0.7))
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
    fn temporal_decay_recent_is_high() {
        let now = Utc::now();
        let factor = super::temporal_decay_factor(&now, &now);
        assert!(factor > 0.99, "factor for now should be ~1.0, got {factor}");
    }

    #[test]
    fn temporal_decay_old_is_low() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(30);
        let factor = super::temporal_decay_factor(&old, &now);
        assert!(factor < 0.1, "factor for 30 days ago should be < 0.1, got {factor}");
    }

    #[test]
    fn temporal_decay_half_life() {
        let now = Utc::now();
        let half = now - chrono::Duration::days(7);
        let factor = super::temporal_decay_factor(&half, &now);
        assert!(
            (factor - 0.5).abs() < 0.01,
            "factor at 7 days should be ~0.5, got {factor}"
        );
    }

    #[test]
    fn cosine_similarity_identical_is_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = super::cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 0.001, "identical vectors should have sim ~1.0, got {sim}");
    }

    #[test]
    fn cosine_similarity_orthogonal_is_zero() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let sim = super::cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001, "orthogonal vectors should have sim ~0.0, got {sim}");
    }

    #[test]
    fn mmr_reduces_duplicate_results() {
        // Create 3 candidates: 2 near-identical + 1 distinct
        let emb_a = vec![1.0_f32; 8];
        let emb_a2 = {
            let mut v = vec![1.0_f32; 8];
            v[0] = 0.99; // Nearly identical to a
            v
        };
        let emb_b = vec![0.0_f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]; // Orthogonal

        let candidates = vec![
            super::MmrCandidate {
                memory: Memory::new("u1".to_string(), MemoryCategory::Fact, "Dark mode".to_string())
                    .with_embedding(emb_a),
                score: 0.9,
            },
            super::MmrCandidate {
                memory: Memory::new("u1".to_string(), MemoryCategory::Fact, "Dark theme".to_string())
                    .with_embedding(emb_a2),
                score: 0.9,
            },
            super::MmrCandidate {
                memory: Memory::new("u1".to_string(), MemoryCategory::Fact, "Works at Acme".to_string())
                    .with_embedding(emb_b),
                score: 0.9,
            },
        ];

        // With equal relevance scores, MMR should prefer diversity
        let results = super::mmr_rerank(candidates, 2, 0.5);
        assert_eq!(results.len(), 2);
        // Both groups should be represented
        let contents: Vec<&str> = results.iter().map(|m| m.content.as_str()).collect();
        assert!(
            contents.contains(&"Works at Acme"),
            "diverse result should be included: {contents:?}"
        );
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
