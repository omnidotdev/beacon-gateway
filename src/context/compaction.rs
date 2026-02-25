//! Session compaction with optional memory flush
//!
//! When a conversation exceeds a message threshold, the oldest messages
//! are summarized via LLM, optionally flushed to long-term memory,
//! then replaced with a concise system summary

use std::sync::Arc;
use std::time::Duration;

use synapse_client::SynapseClient;

use crate::db::{Indexer, MemoryRepo, SessionRepo};
use crate::Result;

/// Configuration for session compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Trigger compaction when message count exceeds this threshold
    pub max_messages_before_compact: usize,
    /// Fraction of messages to summarize (0.0–1.0)
    pub compact_fraction: f64,
    /// Timeout for LLM summarization call
    pub summarize_timeout: Duration,
    /// Whether to flush extracted facts to long-term memory
    pub flush_to_memory: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_messages_before_compact: 40,
            compact_fraction: 0.5,
            summarize_timeout: Duration::from_secs(60),
            flush_to_memory: true,
        }
    }
}

impl CompactionConfig {
    /// Build from environment variables with fallback to defaults
    #[must_use]
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("BEACON_COMPACT_THRESHOLD") {
            if let Ok(n) = val.parse() {
                config.max_messages_before_compact = n;
            }
        }

        if let Ok(val) = std::env::var("BEACON_COMPACT_FLUSH_MEMORY") {
            config.flush_to_memory = !matches!(val.as_str(), "false" | "0" | "no");
        }

        config
    }
}

/// Result of a compaction operation
#[derive(Debug)]
pub struct CompactionResult {
    /// Number of messages removed
    pub messages_removed: usize,
    /// Approximate token count of the summary
    pub summary_tokens: usize,
    /// Number of facts extracted to memory (0 if flush disabled)
    pub facts_extracted: usize,
}

/// Session compactor that summarizes old messages and optionally flushes facts to memory
pub struct SessionCompactor {
    config: CompactionConfig,
    synapse: Arc<SynapseClient>,
    model: String,
}

impl SessionCompactor {
    /// Create a new compactor
    #[must_use]
    pub fn new(config: CompactionConfig, synapse: Arc<SynapseClient>, model: String) -> Self {
        Self {
            config,
            synapse,
            model,
        }
    }

    /// Check if compaction is needed based on message count
    #[must_use]
    pub fn needs_compaction(&self, message_count: usize) -> bool {
        message_count > self.config.max_messages_before_compact
    }

    /// Compact a session by summarizing old messages
    ///
    /// # Errors
    ///
    /// Returns error if summarization fails. Callers should treat
    /// compaction failure as non-fatal and proceed with normal context building.
    pub async fn compact(
        &self,
        session_id: &str,
        session_repo: &SessionRepo,
        _memory_repo: &MemoryRepo,
        indexer: Option<&Indexer>,
        user_id: &str,
    ) -> Result<CompactionResult> {
        // Fetch all messages for the session
        let messages = session_repo.get_messages(session_id, 1000)?;

        if messages.len() <= self.config.max_messages_before_compact {
            return Ok(CompactionResult {
                messages_removed: 0,
                summary_tokens: 0,
                facts_extracted: 0,
            });
        }

        // Select oldest fraction for summarization
        let compact_count =
            (messages.len() as f64 * self.config.compact_fraction).ceil() as usize;
        let compact_count = compact_count.max(1).min(messages.len() - 1);

        let to_summarize = &messages[..compact_count];
        let cutoff_id = &messages[compact_count].id;

        // Build summarization prompt
        let conversation_text = to_summarize
            .iter()
            .map(|m| format!("{}: {}", m.role.as_display_str(), m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Summarize the following conversation concisely, preserving key facts, \
             decisions, and user preferences. Keep it under 200 words.\n\n{conversation_text}"
        );

        // Call LLM for summarization (non-streaming)
        let request = synapse_client::ChatRequest {
            model: self.model.clone(),
            messages: vec![synapse_client::Message::user(&prompt)],
            stream: false,
            temperature: Some(0.3),
            top_p: None,
            max_tokens: Some(300),
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let summary = tokio::time::timeout(
            self.config.summarize_timeout,
            self.synapse.chat_completion(&request),
        )
        .await
        .map_err(|_| crate::Error::Tool("compaction summarization timed out".to_string()))?
        .map_err(|e| crate::Error::Tool(format!("compaction summarization failed: {e}")))?;

        let summary_text = summary
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let summary_tokens = summary_text.split_whitespace().count();

        // Optionally extract facts to memory
        let mut facts_extracted = 0;
        if self.config.flush_to_memory {
            if let Some(indexer) = indexer {
                match indexer
                    .index_conversation(user_id, &conversation_text, Some(session_id), None)
                    .await
                {
                    Ok(memories) => {
                        facts_extracted = memories.len();
                        if facts_extracted > 0 {
                            tracing::info!(
                                session = session_id,
                                facts = facts_extracted,
                                "compaction flushed facts to memory"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            session = session_id,
                            error = %e,
                            "compaction memory flush failed"
                        );
                    }
                }
            }
        }

        // Delete summarized messages and insert summary
        let messages_removed = session_repo.delete_messages_before(session_id, cutoff_id)?;
        let summary_header = format!("[Conversation summary]\n{summary_text}");
        session_repo.insert_summary(session_id, &summary_header)?;

        tracing::info!(
            session = session_id,
            removed = messages_removed,
            summary_tokens,
            facts = facts_extracted,
            "session compacted"
        );

        Ok(CompactionResult {
            messages_removed,
            summary_tokens,
            facts_extracted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_compaction_below_threshold_false() {
        let config = CompactionConfig {
            max_messages_before_compact: 40,
            ..Default::default()
        };
        let synapse = synapse_client::SynapseClient::new("http://localhost:1234").unwrap();
        let compactor = SessionCompactor::new(config, Arc::new(synapse), "test".to_string());

        assert!(!compactor.needs_compaction(10));
        assert!(!compactor.needs_compaction(40));
    }

    #[test]
    fn needs_compaction_above_threshold_true() {
        let config = CompactionConfig {
            max_messages_before_compact: 40,
            ..Default::default()
        };
        let synapse = synapse_client::SynapseClient::new("http://localhost:1234").unwrap();
        let compactor = SessionCompactor::new(config, Arc::new(synapse), "test".to_string());

        assert!(compactor.needs_compaction(41));
        assert!(compactor.needs_compaction(100));
    }

    #[test]
    fn delete_messages_removes_old() {
        let pool = crate::db::init_memory().unwrap();
        let session_repo = SessionRepo::new(pool.clone());

        // Create user and session
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO users (id) VALUES ('compact-user')", []).unwrap();
        drop(conn);

        let session = session_repo
            .find_or_create("compact-user", "test", "compact-chan", "orin")
            .unwrap();

        // Add messages with small delays
        let m1 = session_repo
            .add_message(&session.id, crate::db::MessageRole::User, "First")
            .unwrap();
        let _m2 = session_repo
            .add_message(&session.id, crate::db::MessageRole::Assistant, "Second")
            .unwrap();
        let _m3 = session_repo
            .add_message(&session.id, crate::db::MessageRole::User, "Third")
            .unwrap();

        // Delete before m1 should delete nothing (m1 is the earliest)
        // We need a message that is after some others
        let count = session_repo.message_count(&session.id).unwrap();
        assert_eq!(count, 3);

        // Delete messages before the third message
        let deleted = session_repo.delete_messages_before(&session.id, &_m3.id).unwrap();
        // m1 and m2 should be deleted (created_at < m3.created_at)
        assert!(deleted >= 1, "expected at least 1 deleted, got {deleted}");

        // Verify remaining messages
        let remaining = session_repo.get_messages(&session.id, 10).unwrap();
        // At least the third message should remain
        assert!(
            remaining.iter().any(|m| m.content == "Third"),
            "Third message should remain"
        );
    }

    #[test]
    fn insert_summary_adds_system_message() {
        let pool = crate::db::init_memory().unwrap();
        let session_repo = SessionRepo::new(pool.clone());

        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO users (id) VALUES ('summary-user')", []).unwrap();
        drop(conn);

        let session = session_repo
            .find_or_create("summary-user", "test", "summary-chan", "orin")
            .unwrap();

        session_repo
            .add_message(&session.id, crate::db::MessageRole::User, "Hello")
            .unwrap();

        let summary = session_repo
            .insert_summary(&session.id, "Summary of prior conversation")
            .unwrap();

        assert_eq!(summary.role, crate::db::MessageRole::System);
        assert!(summary.content.contains("Summary of prior conversation"));

        // Summary should appear before the user message
        let messages = session_repo.get_messages(&session.id, 10).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, crate::db::MessageRole::System);
        assert_eq!(messages[1].role, crate::db::MessageRole::User);
    }
}
