//! HTTP client for syncing memories with the cloud API

use serde::{Deserialize, Serialize};

use crate::db::{DbPool, Memory, MemoryCategory, MemoryRepo};
use crate::{Error, Result};

use super::merge::merge_memory;

/// Client for syncing memories with the Beacon cloud API
#[derive(Clone)]
pub struct SyncClient {
    api_url: String,
    device_id: String,
    client: reqwest::Client,
    auth_token: Option<String>,
}

/// Memory payload sent to/received from the API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemorySyncPayload {
    gateway_memory_id: String,
    category: String,
    content: String,
    content_hash: Option<String>,
    tags: Vec<String>,
    pinned: bool,
    access_count: u32,
    source_session_id: Option<String>,
    source_channel: Option<String>,
    origin_device_id: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

/// Response from the `pushMemories` mutation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushMemoriesResponse {
    data: Option<PushMemoriesData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushMemoriesData {
    push_memories: PushResult,
}

#[derive(Debug, Deserialize)]
struct PushResult {
    pushed: usize,
    merged: usize,
}

/// Response from the `memoriesSince` query
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullMemoriesResponse {
    data: Option<PullMemoriesData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullMemoriesData {
    memories_since: MemoriesSinceResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoriesSinceResult {
    memories: Vec<RemoteMemory>,
    cursor: String,
    has_more: bool,
}

/// Memory as returned from the cloud API
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteMemory {
    id: String,
    gateway_memory_id: String,
    category: String,
    content: String,
    content_hash: Option<String>,
    tags: serde_json::Value,
    pinned: bool,
    access_count: i32,
    source_session_id: Option<String>,
    source_channel: Option<String>,
    origin_device_id: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

impl SyncClient {
    /// Create a new sync client
    #[must_use]
    pub fn new(api_url: &str, device_id: &str) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            client: reqwest::Client::new(),
            auth_token: None,
        }
    }

    /// Set the authentication token for API calls
    #[must_use]
    pub fn with_auth_token(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Push local changes to the cloud API
    ///
    /// Queries memories where `updated_at > synced_at` (or `synced_at` is NULL),
    /// POSTs to the API's `pushMemories` GraphQL mutation, and updates local
    /// `synced_at` on success.
    ///
    /// # Errors
    ///
    /// Returns error if database query or API call fails
    pub async fn push_changes(&self, db: &DbPool) -> Result<usize> {
        let repo = MemoryRepo::new(db.clone());
        let unsynced = repo.unsynced()?;

        if unsynced.is_empty() {
            tracing::debug!("no unsynced memories to push");
            return Ok(0);
        }

        let payloads: Vec<MemorySyncPayload> = unsynced
            .iter()
            .map(|m| MemorySyncPayload {
                gateway_memory_id: m.id.clone(),
                category: m.category.to_string(),
                content: m.content.clone(),
                content_hash: m.content_hash.clone(),
                tags: m.tags.clone(),
                pinned: m.pinned,
                access_count: m.access_count,
                source_session_id: m.source_session_id.clone(),
                source_channel: m.source_channel.clone(),
                origin_device_id: m.origin_device_id.clone()
                    .or_else(|| Some(self.device_id.clone())),
                created_at: m.created_at.to_rfc3339(),
                updated_at: m.updated_at.clone(),
                deleted_at: m.deleted_at.clone(),
            })
            .collect();

        let input_json = serde_json::to_string(&payloads)
            .map_err(|e| Error::Database(format!("failed to serialize push payload: {e}")))?;

        let query = format!(
            r#"mutation {{ pushMemories(input: {input_json}) {{ pushed merged }} }}"#
        );

        let response = self.graphql_request(&query).await?;
        let parsed: PushMemoriesResponse = response.json().await?;

        if let Some(errors) = parsed.errors {
            let msgs: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
            return Err(Error::Database(format!("push sync errors: {}", msgs.join(", "))));
        }

        let result = parsed
            .data
            .ok_or_else(|| Error::Database("push response missing data".to_string()))?
            .push_memories;

        // Mark pushed memories as synced
        let ids: Vec<&str> = unsynced.iter().map(|m| m.id.as_str()).collect();
        repo.mark_synced(&ids)?;

        let total = result.pushed + result.merged;
        tracing::info!(
            pushed = result.pushed,
            merged = result.merged,
            "pushed memory changes to cloud"
        );

        Ok(total)
    }

    /// Pull changes from the cloud API
    ///
    /// Calls the API's `memoriesSince` query, upserts locally with LWW merge,
    /// and marks pulled memories for re-embedding.
    ///
    /// # Errors
    ///
    /// Returns error if API call or database operation fails
    pub async fn pull_changes(&self, db: &DbPool) -> Result<usize> {
        let repo = MemoryRepo::new(db.clone());
        let mut total_pulled = 0;
        let mut cursor = String::new();

        loop {
            let since = if cursor.is_empty() {
                // Use the most recent synced_at as the starting point
                "1970-01-01T00:00:00Z".to_string()
            } else {
                cursor.clone()
            };

            let query = format!(
                r#"query {{ memoriesSince(since: "{since}", deviceId: "{}") {{ memories {{ id gatewayMemoryId category content contentHash tags pinned accessCount sourceSessionId sourceChannel originDeviceId createdAt updatedAt deletedAt }} cursor hasMore }} }}"#,
                self.device_id
            );

            let response = self.graphql_request(&query).await?;
            let parsed: PullMemoriesResponse = response.json().await?;

            if let Some(errors) = parsed.errors {
                let msgs: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
                return Err(Error::Database(format!("pull sync errors: {}", msgs.join(", "))));
            }

            let result = parsed
                .data
                .ok_or_else(|| Error::Database("pull response missing data".to_string()))?
                .memories_since;

            for remote in &result.memories {
                let memory = remote_to_memory(remote, &self.device_id);

                // Check if we already have this memory locally
                if let Some(local) = repo.get_without_access_update(&memory.id)? {
                    let merged = merge_memory(&local, &memory);
                    repo.upsert_from_remote(&merged)?;
                } else {
                    repo.upsert_from_remote(&memory)?;
                }

                total_pulled += 1;
            }

            cursor = result.cursor;

            if !result.has_more {
                break;
            }
        }

        if total_pulled > 0 {
            tracing::info!(count = total_pulled, "pulled memory changes from cloud");
        }

        Ok(total_pulled)
    }

    /// Run a full sync: push local changes, then pull remote changes
    ///
    /// # Errors
    ///
    /// Returns error if push or pull fails
    pub async fn full_sync(&self, db: &DbPool) -> Result<()> {
        let pushed = self.push_changes(db).await?;
        let pulled = self.pull_changes(db).await?;

        if pushed > 0 || pulled > 0 {
            tracing::info!(pushed, pulled, "memory sync complete");
        }

        Ok(())
    }

    /// Send a GraphQL request to the API
    async fn graphql_request(&self, query: &str) -> Result<reqwest::Response> {
        let url = format!("{}/graphql", self.api_url);

        let body = serde_json::json!({
            "query": query,
        });

        let mut request = self.client.post(&url).json(&body);

        if let Some(ref token) = self.auth_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Database(format!(
                "sync API error {status}: {body}"
            )));
        }

        Ok(response)
    }
}

/// Convert a remote memory from the API into a local Memory struct
fn remote_to_memory(remote: &RemoteMemory, _device_id: &str) -> Memory {
    use chrono::{DateTime, Utc};

    let category = match remote.category.as_str() {
        "preference" => MemoryCategory::Preference,
        "fact" => MemoryCategory::Fact,
        "correction" => MemoryCategory::Correction,
        _ => MemoryCategory::General,
    };

    let tags: Vec<String> = match &remote.tags {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        serde_json::Value::String(s) => {
            serde_json::from_str(s).unwrap_or_default()
        }
        _ => Vec::new(),
    };

    let created_at = DateTime::parse_from_rfc3339(&remote.created_at)
        .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

    let accessed_at = created_at;

    Memory {
        id: remote.gateway_memory_id.clone(),
        user_id: String::new(), // Filled in by caller context
        category,
        content: remote.content.clone(),
        tags,
        pinned: remote.pinned,
        access_count: u32::try_from(remote.access_count).unwrap_or(0),
        created_at,
        accessed_at,
        embedding: None, // Embeddings are device-local, will be re-generated
        source_session_id: remote.source_session_id.clone(),
        source_channel: remote.source_channel.clone(),
        content_hash: remote.content_hash.clone(),
        origin_device_id: remote.origin_device_id.clone(),
        updated_at: remote.updated_at.clone(),
        deleted_at: remote.deleted_at.clone(),
        synced_at: None,
        cloud_id: Some(remote.id.clone()),
    }
}
