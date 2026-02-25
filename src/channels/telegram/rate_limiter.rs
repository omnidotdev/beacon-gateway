//! Per-chat rate limiter for Telegram API edit operations

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-chat rate limiter for Telegram API edit operations
#[derive(Debug, Clone)]
pub struct TelegramRateLimiter {
    /// Minimum interval between edits per chat
    interval: Duration,
    /// Last edit timestamp per chat
    last_edit: Arc<Mutex<HashMap<String, Instant>>>,
}

impl TelegramRateLimiter {
    /// Create a rate limiter with the given minimum interval between edits per chat
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_edit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if an edit is allowed for the given chat. Returns true if allowed.
    pub fn check(&self, chat_id: &str) -> bool {
        let mut map = self.last_edit.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        if let Some(last) = map.get(chat_id) {
            if now.duration_since(*last) < self.interval {
                return false;
            }
        }

        map.insert(chat_id.to_string(), now);
        true
    }

    /// Record a 429 response — push the effective interval forward for this chat
    pub fn backoff(&self, chat_id: &str) {
        let mut map = self.last_edit.lock().unwrap_or_else(|e| e.into_inner());
        let future = Instant::now() + self.interval;
        map.insert(chat_id.to_string(), future);
    }
}
