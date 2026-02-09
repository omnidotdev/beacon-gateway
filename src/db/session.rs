//! Session repository for CRUD operations

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

/// A conversation session
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub channel: String,
    pub channel_id: String,
    pub persona_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A message in a session
#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
    /// Thread identifier for grouping related messages
    pub thread_id: Option<String>,
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// Session repository
#[derive(Clone)]
pub struct SessionRepo {
    pool: DbPool,
}

impl SessionRepo {
    /// Create a new session repository
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Find or create a session for a channel conversation
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn find_or_create(
        &self,
        user_id: &str,
        channel: &str,
        channel_id: &str,
        persona_id: &str,
    ) -> Result<Session> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        // Try to find existing session
        let existing: Option<Session> = conn
            .query_row(
                "SELECT id, user_id, channel, channel_id, persona_id, created_at, updated_at
                 FROM sessions WHERE channel = ?1 AND channel_id = ?2",
                [channel, channel_id],
                |row| {
                    Ok(Session {
                        id: row.get(0)?,
                        user_id: row.get(1)?,
                        channel: row.get(2)?,
                        channel_id: row.get(3)?,
                        persona_id: row.get(4)?,
                        created_at: parse_datetime(&row.get::<_, String>(5)?),
                        updated_at: parse_datetime(&row.get::<_, String>(6)?),
                    })
                },
            )
            .ok();

        if let Some(session) = existing {
            return Ok(session);
        }

        // Create new session
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO sessions (id, user_id, channel, channel_id, persona_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            [&id, user_id, channel, channel_id, persona_id, &now],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(Session {
            id,
            user_id: user_id.to_string(),
            channel: channel.to_string(),
            channel_id: channel_id.to_string(),
            persona_id: persona_id.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// List all sessions
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_all(&self) -> Result<Vec<Session>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, channel, channel_id, persona_id, created_at, updated_at
                 FROM sessions ORDER BY updated_at DESC",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let sessions = stmt
            .query_map([], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    channel: row.get(2)?,
                    channel_id: row.get(3)?,
                    persona_id: row.get(4)?,
                    created_at: parse_datetime(&row.get::<_, String>(5)?),
                    updated_at: parse_datetime(&row.get::<_, String>(6)?),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(sessions)
    }

    /// Add a message to a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn add_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<Message> {
        self.add_message_with_thread(session_id, role, content, None)
    }

    /// Add a message to a session with thread context
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn add_message_with_thread(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<Message> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at, thread_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![&id, session_id, role.as_str(), content, &now_str, thread_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        // Update session updated_at
        conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            [&now_str, session_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(Message {
            id,
            session_id: session_id.to_string(),
            role,
            content: content.to_string(),
            created_at: now,
            thread_id: thread_id.map(String::from),
        })
    }

    /// Get recent messages for a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_messages(&self, session_id: &str, limit: usize) -> Result<Vec<Message>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, role, content, created_at, thread_id
                 FROM messages WHERE session_id = ?1
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let messages = stmt
            .query_map([session_id, &limit.to_string()], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: MessageRole::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(MessageRole::User),
                    content: row.get(3)?,
                    created_at: parse_datetime(&row.get::<_, String>(4)?),
                    thread_id: row.get(5)?,
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        Ok(messages)
    }

    /// Get recent messages for a specific thread within a session
    ///
    /// If `thread_id` is None, returns messages that have no thread (root-level messages)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_messages_in_thread(
        &self,
        session_id: &str,
        thread_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let messages = if let Some(tid) = thread_id {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, role, content, created_at, thread_id
                     FROM messages WHERE session_id = ?1 AND thread_id = ?2
                     ORDER BY created_at DESC LIMIT ?3",
                )
                .map_err(|e| Error::Database(e.to_string()))?;

            #[allow(clippy::cast_possible_wrap)]
            stmt.query_map(rusqlite::params![session_id, tid, limit as i64], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: MessageRole::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(MessageRole::User),
                    content: row.get(3)?,
                    created_at: parse_datetime(&row.get::<_, String>(4)?),
                    thread_id: row.get(5)?,
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>()
        } else {
            // Get messages with no thread (root level)
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, role, content, created_at, thread_id
                     FROM messages WHERE session_id = ?1 AND thread_id IS NULL
                     ORDER BY created_at DESC LIMIT ?2",
                )
                .map_err(|e| Error::Database(e.to_string()))?;

            #[allow(clippy::cast_possible_wrap)]
            stmt.query_map(rusqlite::params![session_id, limit as i64], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: MessageRole::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(MessageRole::User),
                    content: row.get(3)?,
                    created_at: parse_datetime(&row.get::<_, String>(4)?),
                    thread_id: row.get(5)?,
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>()
        };

        // Reverse to get chronological order
        Ok(messages.into_iter().rev().collect())
    }

    /// Count messages in a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn message_count(&self, session_id: &str) -> Result<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(usize::try_from(count).unwrap_or(0))
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup() -> SessionRepo {
        let pool = init_memory().unwrap();

        // Create a test user
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO users (id) VALUES ('test-user')", [])
            .unwrap();

        SessionRepo::new(pool)
    }

    #[test]
    fn test_find_or_create_session() {
        let repo = setup();

        let session = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .unwrap();

        assert_eq!(session.channel, "discord");
        assert_eq!(session.channel_id, "channel-123");

        // Should return same session
        let session2 = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .unwrap();

        assert_eq!(session.id, session2.id);
    }

    #[test]
    fn test_add_and_get_messages() {
        let repo = setup();

        let session = repo
            .find_or_create("test-user", "voice", "local", "orin")
            .unwrap();

        repo.add_message(&session.id, MessageRole::User, "Hello")
            .unwrap();
        repo.add_message(&session.id, MessageRole::Assistant, "Hi there!")
            .unwrap();

        let messages = repo.get_messages(&session.id, 10).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Hello");
        assert_eq!(messages[1].content, "Hi there!");
        assert!(messages[0].thread_id.is_none());
    }

    #[test]
    fn test_thread_messages() {
        let repo = setup();

        let session = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .unwrap();

        // Add root-level message
        repo.add_message(&session.id, MessageRole::User, "Root message")
            .unwrap();

        // Add threaded messages
        repo.add_message_with_thread(
            &session.id,
            MessageRole::User,
            "Thread message 1",
            Some("thread-abc"),
        )
        .unwrap();
        repo.add_message_with_thread(
            &session.id,
            MessageRole::Assistant,
            "Thread reply 1",
            Some("thread-abc"),
        )
        .unwrap();

        // Get all messages
        let all = repo.get_messages(&session.id, 10).unwrap();
        assert_eq!(all.len(), 3);

        // Get only root-level messages
        let root = repo.get_messages_in_thread(&session.id, None, 10).unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].content, "Root message");

        // Get only threaded messages
        let threaded = repo
            .get_messages_in_thread(&session.id, Some("thread-abc"), 10)
            .unwrap();
        assert_eq!(threaded.len(), 2);
        assert_eq!(threaded[0].content, "Thread message 1");
        assert_eq!(threaded[1].content, "Thread reply 1");
    }

    #[test]
    fn test_message_count() {
        let repo = setup();

        let session = repo
            .find_or_create("test-user", "slack", "channel-456", "orin")
            .unwrap();

        assert_eq!(repo.message_count(&session.id).unwrap(), 0);

        repo.add_message(&session.id, MessageRole::User, "Test")
            .unwrap();

        assert_eq!(repo.message_count(&session.id).unwrap(), 1);
    }
}
