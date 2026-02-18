//! Last-writer-wins merge logic for memory sync

use crate::db::Memory;

/// Merge a local memory with a remote memory using LWW semantics
///
/// - Fields use last-writer-wins by `updated_at`
/// - `access_count` uses `max()` to avoid inflation
/// - Soft-deletes propagate (if either side is deleted, the result is deleted)
#[must_use]
pub fn merge_memory(local: &Memory, remote: &Memory) -> Memory {
    let local_wins = local.updated_at >= remote.updated_at;

    let (winner, loser) = if local_wins {
        (local, remote)
    } else {
        (remote, local)
    };

    let mut merged = winner.clone();

    // access_count: take the max to avoid inflation from summing
    merged.access_count = std::cmp::max(local.access_count, remote.access_count);

    // Propagate soft-deletes: if either side has deleted_at, use the earliest
    merged.deleted_at = match (&local.deleted_at, &remote.deleted_at) {
        (Some(l), Some(r)) => Some(std::cmp::min(l.clone(), r.clone())),
        (Some(d), None) | (None, Some(d)) => Some(d.clone()),
        (None, None) => None,
    };

    // Preserve the original created_at (earliest)
    if loser.created_at < winner.created_at {
        merged.created_at = loser.created_at;
    }

    // Keep the cloud_id if either side has one
    if merged.cloud_id.is_none() {
        merged.cloud_id = loser.cloud_id.clone();
    }

    merged
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::db::{Memory, MemoryCategory};

    fn make_memory(content: &str, updated_at: &str) -> Memory {
        let now = Utc::now();
        Memory {
            id: format!("mem_test_{content}"),
            user_id: "user_1".to_string(),
            category: MemoryCategory::Fact,
            content: content.to_string(),
            tags: vec![],
            pinned: false,
            access_count: 1,
            created_at: now,
            accessed_at: now,
            embedding: None,
            source_session_id: None,
            source_channel: None,
            content_hash: Some(Memory::compute_content_hash(content)),
            origin_device_id: Some("device_a".to_string()),
            updated_at: updated_at.to_string(),
            deleted_at: None,
            synced_at: None,
            cloud_id: None,
        }
    }

    #[test]
    fn test_lww_remote_wins() {
        let local = make_memory("old content", "2025-01-01T00:00:00Z");
        let remote = make_memory("new content", "2025-01-02T00:00:00Z");

        let merged = merge_memory(&local, &remote);
        assert_eq!(merged.content, "new content");
        assert_eq!(merged.updated_at, "2025-01-02T00:00:00Z");
    }

    #[test]
    fn test_lww_local_wins() {
        let local = make_memory("newer content", "2025-01-03T00:00:00Z");
        let remote = make_memory("older content", "2025-01-01T00:00:00Z");

        let merged = merge_memory(&local, &remote);
        assert_eq!(merged.content, "newer content");
    }

    #[test]
    fn test_access_count_max() {
        let mut local = make_memory("content", "2025-01-01T00:00:00Z");
        local.access_count = 5;

        let mut remote = make_memory("content", "2025-01-02T00:00:00Z");
        remote.access_count = 3;

        let merged = merge_memory(&local, &remote);
        assert_eq!(merged.access_count, 5);
    }

    #[test]
    fn test_deleted_propagates() {
        let local = make_memory("content", "2025-01-01T00:00:00Z");
        let mut remote = make_memory("content", "2025-01-02T00:00:00Z");
        remote.deleted_at = Some("2025-01-02T12:00:00Z".to_string());

        let merged = merge_memory(&local, &remote);
        assert!(merged.deleted_at.is_some());
    }

    #[test]
    fn test_cloud_id_preserved() {
        let mut local = make_memory("content", "2025-01-01T00:00:00Z");
        local.cloud_id = Some("cloud_123".to_string());

        let remote = make_memory("content", "2025-01-02T00:00:00Z");

        let merged = merge_memory(&local, &remote);
        assert_eq!(merged.cloud_id.as_deref(), Some("cloud_123"));
    }
}
