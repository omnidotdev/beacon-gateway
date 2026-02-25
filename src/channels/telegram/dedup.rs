//! Telegram update deduplication cache

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Default dedup TTL (5 minutes)
const DEDUP_TTL_SECS: u64 = 300;

/// Maximum dedup cache entries
const DEDUP_MAX_ENTRIES: usize = 2000;

/// Telegram update deduplication cache
///
/// Prevents processing the same webhook update or polling result twice.
/// Uses a TTL-based eviction strategy with a hard cap on entries.
#[derive(Debug)]
pub struct UpdateDedup {
    cache: HashMap<String, Instant>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for UpdateDedup {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(DEDUP_TTL_SECS),
            max_entries: DEDUP_MAX_ENTRIES,
        }
    }
}

impl UpdateDedup {
    /// Check if the given key has been seen recently.
    ///
    /// Returns `true` if this is a duplicate (already seen within TTL).
    /// Returns `false` on first sight and records the key.
    pub fn is_duplicate(&mut self, key: &str) -> bool {
        let now = Instant::now();

        // Evict expired entries periodically (when at capacity)
        if self.cache.len() >= self.max_entries {
            self.cache.retain(|_, ts| now.duration_since(*ts) < self.ttl);
        }

        // If still at capacity after eviction, remove oldest entry
        if self.cache.len() >= self.max_entries {
            if let Some(oldest_key) = self
                .cache
                .iter()
                .min_by_key(|(_, ts)| *ts)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&oldest_key);
            }
        }

        if let Some(ts) = self.cache.get(key) {
            if now.duration_since(*ts) < self.ttl {
                return true;
            }
        }

        self.cache.insert(key.to_string(), now);
        false
    }
}
