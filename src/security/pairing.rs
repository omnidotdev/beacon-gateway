//! DM pairing and allowlist security
//!
//! Provides three modes of access control for incoming DMs:
//! - Open: Accept all DMs (no security)
//! - Pairing: New senders must enter a pairing code to be approved
//! - Allowlist: Only pre-approved sender IDs can message

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::DbPool;
use crate::{Error, Result};

/// Pairing code length
const PAIRING_CODE_LENGTH: usize = 6;

/// Pairing code valid duration in minutes
const PAIRING_CODE_EXPIRY_MINUTES: i64 = 10;

/// DM access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmPolicy {
    /// Accept all DMs without authentication
    #[default]
    Open,

    /// New senders must enter a pairing code
    Pairing,

    /// Only pre-approved sender IDs can message
    Allowlist,
}

impl DmPolicy {
    /// Parse from string representation
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pairing" => Self::Pairing,
            "allowlist" | "whitelist" => Self::Allowlist,
            _ => Self::Open,
        }
    }
}

impl std::fmt::Display for DmPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Pairing => write!(f, "pairing"),
            Self::Allowlist => write!(f, "allowlist"),
        }
    }
}

/// A paired user record
#[derive(Debug, Clone)]
pub struct PairedUser {
    pub id: String,
    pub sender_id: String,
    pub channel: String,
    pub paired_at: DateTime<Utc>,
    pub pairing_code: Option<String>,
    pub code_expires_at: Option<DateTime<Utc>>,
}

/// Manages DM pairing and access control
#[derive(Clone)]
pub struct PairingManager {
    policy: DmPolicy,
    pool: DbPool,
}

impl PairingManager {
    /// Create a new pairing manager
    #[must_use]
    pub const fn new(policy: DmPolicy, pool: DbPool) -> Self {
        Self { policy, pool }
    }

    /// Get the current policy
    #[must_use]
    pub const fn policy(&self) -> DmPolicy {
        self.policy
    }

    /// Check if a sender is allowed to message
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn is_allowed(&self, sender_id: &str, channel: &str) -> Result<bool> {
        match self.policy {
            DmPolicy::Open => Ok(true),
            DmPolicy::Pairing | DmPolicy::Allowlist => self.is_paired(sender_id, channel),
        }
    }

    /// Check if sender is paired (has a valid entry without pending code)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn is_paired(&self, sender_id: &str, channel: &str) -> Result<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM paired_users
                 WHERE sender_id = ?1 AND channel = ?2 AND pairing_code IS NULL",
                [sender_id, channel],
                |_| Ok(true),
            )
            .unwrap_or(false);

        Ok(exists)
    }

    /// Generate a pairing code for a new sender
    ///
    /// Returns the code if generated, or None if sender is already paired
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn generate_pairing_code(&self, sender_id: &str, channel: &str) -> Result<Option<String>> {
        // Check if already paired
        if self.is_paired(sender_id, channel)? {
            return Ok(None);
        }

        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        // Generate a 6-character alphanumeric code
        let code = generate_code(PAIRING_CODE_LENGTH);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(PAIRING_CODE_EXPIRY_MINUTES);

        // Check if pending code already exists
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM paired_users WHERE sender_id = ?1 AND channel = ?2",
                [sender_id, channel],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing_id {
            // Update existing pending record
            conn.execute(
                "UPDATE paired_users SET pairing_code = ?1, code_expires_at = ?2 WHERE id = ?3",
                [&code, &expires_at.to_rfc3339(), &id],
            )
            .map_err(|e| Error::Database(e.to_string()))?;
        } else {
            // Create new pending record
            let id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO paired_users (id, sender_id, channel, paired_at, pairing_code, code_expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                [
                    &id,
                    sender_id,
                    channel,
                    &now.to_rfc3339(),
                    &code,
                    &expires_at.to_rfc3339(),
                ],
            )
            .map_err(|e| Error::Database(e.to_string()))?;
        }

        tracing::debug!(sender_id, channel, "generated pairing code");
        Ok(Some(code))
    }

    /// Verify a pairing code and approve the sender
    ///
    /// Returns true if code is valid and sender is now paired
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn verify_pairing(&self, sender_id: &str, channel: &str, code: &str) -> Result<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        // Find pending pairing with matching code
        let record: Option<(String, String)> = conn
            .query_row(
                "SELECT id, code_expires_at FROM paired_users
                 WHERE sender_id = ?1 AND channel = ?2 AND pairing_code = ?3",
                [sender_id, channel, code],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let Some((id, expires_at_str)) = record else {
            tracing::debug!(sender_id, channel, "invalid pairing code");
            return Ok(false);
        };

        // Check expiry
        if let Ok(expires_at) = DateTime::parse_from_rfc3339(&expires_at_str) {
            if Utc::now() > expires_at.with_timezone(&Utc) {
                tracing::debug!(sender_id, channel, "pairing code expired");
                return Ok(false);
            }
        }

        // Clear the code to mark as paired
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE paired_users SET pairing_code = NULL, code_expires_at = NULL, paired_at = ?1 WHERE id = ?2",
            [&now, &id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        tracing::info!(sender_id, channel, "sender paired successfully");
        Ok(true)
    }

    /// Add a sender directly to the allowlist (bypassing pairing flow)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn add_to_allowlist(&self, sender_id: &str, channel: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO paired_users (id, sender_id, channel, paired_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sender_id, channel) DO UPDATE SET pairing_code = NULL, code_expires_at = NULL",
            [&id, sender_id, channel, &now],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        tracing::info!(sender_id, channel, "added to allowlist");
        Ok(())
    }

    /// Remove a sender from the allowlist
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn remove_from_allowlist(&self, sender_id: &str, channel: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        conn.execute(
            "DELETE FROM paired_users WHERE sender_id = ?1 AND channel = ?2",
            [sender_id, channel],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        tracing::info!(sender_id, channel, "removed from allowlist");
        Ok(())
    }

    /// List all paired users for a channel
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_paired(&self, channel: &str) -> Result<Vec<PairedUser>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, sender_id, channel, paired_at, pairing_code, code_expires_at
                 FROM paired_users
                 WHERE channel = ?1 AND pairing_code IS NULL
                 ORDER BY paired_at DESC",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let users = stmt
            .query_map([channel], |row| {
                Ok(PairedUser {
                    id: row.get(0)?,
                    sender_id: row.get(1)?,
                    channel: row.get(2)?,
                    paired_at: parse_datetime(&row.get::<_, String>(3)?),
                    pairing_code: row.get(4)?,
                    code_expires_at: row
                        .get::<_, Option<String>>(5)?
                        .map(|s| parse_datetime(&s)),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(users)
    }

    /// List all paired users across all channels
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list_all_paired(&self) -> Result<Vec<PairedUser>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, sender_id, channel, paired_at, pairing_code, code_expires_at
                 FROM paired_users
                 WHERE pairing_code IS NULL
                 ORDER BY paired_at DESC",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let users = stmt
            .query_map([], |row| {
                Ok(PairedUser {
                    id: row.get(0)?,
                    sender_id: row.get(1)?,
                    channel: row.get(2)?,
                    paired_at: parse_datetime(&row.get::<_, String>(3)?),
                    pairing_code: row.get(4)?,
                    code_expires_at: row
                        .get::<_, Option<String>>(5)?
                        .map(|s| parse_datetime(&s)),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(users)
    }
}

/// Generate a random alphanumeric code
fn generate_code(length: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut code = String::with_capacity(length);
    let hasher_builder = RandomState::new();

    for i in 0..length {
        let mut hasher = hasher_builder.build_hasher();
        hasher.write_usize(i);
        hasher.write_u128(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos());
        #[allow(clippy::cast_possible_truncation)]
        let idx = hasher.finish() as usize % CHARSET.len();
        code.push(CHARSET[idx] as char);
    }

    code
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup(policy: DmPolicy) -> PairingManager {
        let pool = init_memory().unwrap();
        PairingManager::new(policy, pool)
    }

    #[test]
    fn test_open_policy_allows_all() {
        let manager = setup(DmPolicy::Open);
        assert!(manager.is_allowed("anyone", "discord").unwrap());
    }

    #[test]
    fn test_allowlist_denies_unknown() {
        let manager = setup(DmPolicy::Allowlist);
        assert!(!manager.is_allowed("unknown", "discord").unwrap());
    }

    #[test]
    fn test_add_to_allowlist() {
        let manager = setup(DmPolicy::Allowlist);

        manager.add_to_allowlist("user123", "discord").unwrap();
        assert!(manager.is_allowed("user123", "discord").unwrap());

        // Different channel should not be allowed
        assert!(!manager.is_allowed("user123", "slack").unwrap());
    }

    #[test]
    fn test_pairing_flow() {
        let manager = setup(DmPolicy::Pairing);

        // Initially not allowed
        assert!(!manager.is_allowed("newuser", "telegram").unwrap());

        // Generate code
        let code = manager
            .generate_pairing_code("newuser", "telegram")
            .unwrap()
            .unwrap();
        assert_eq!(code.len(), PAIRING_CODE_LENGTH);

        // Still not allowed with pending code
        assert!(!manager.is_allowed("newuser", "telegram").unwrap());

        // Wrong code fails
        assert!(!manager.verify_pairing("newuser", "telegram", "WRONG1").unwrap());

        // Correct code succeeds
        assert!(manager.verify_pairing("newuser", "telegram", &code).unwrap());

        // Now allowed
        assert!(manager.is_allowed("newuser", "telegram").unwrap());

        // Code can't be reused
        assert!(!manager.verify_pairing("newuser", "telegram", &code).unwrap());
    }

    #[test]
    fn test_already_paired_returns_none() {
        let manager = setup(DmPolicy::Pairing);

        manager.add_to_allowlist("existinguser", "slack").unwrap();

        // Should return None for already paired user
        let code = manager
            .generate_pairing_code("existinguser", "slack")
            .unwrap();
        assert!(code.is_none());
    }

    #[test]
    fn test_remove_from_allowlist() {
        let manager = setup(DmPolicy::Allowlist);

        manager.add_to_allowlist("user456", "signal").unwrap();
        assert!(manager.is_allowed("user456", "signal").unwrap());

        manager.remove_from_allowlist("user456", "signal").unwrap();
        assert!(!manager.is_allowed("user456", "signal").unwrap());
    }

    #[test]
    fn test_list_paired() {
        let manager = setup(DmPolicy::Allowlist);

        manager.add_to_allowlist("alice", "discord").unwrap();
        manager.add_to_allowlist("bob", "discord").unwrap();
        manager.add_to_allowlist("charlie", "slack").unwrap();

        let discord_users = manager.list_paired("discord").unwrap();
        assert_eq!(discord_users.len(), 2);

        let all_users = manager.list_all_paired().unwrap();
        assert_eq!(all_users.len(), 3);
    }

    #[test]
    fn test_generate_code_format() {
        let code = generate_code(6);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_dm_policy_from_str() {
        assert_eq!(DmPolicy::from_str("open"), DmPolicy::Open);
        assert_eq!(DmPolicy::from_str("pairing"), DmPolicy::Pairing);
        assert_eq!(DmPolicy::from_str("allowlist"), DmPolicy::Allowlist);
        assert_eq!(DmPolicy::from_str("whitelist"), DmPolicy::Allowlist);
        assert_eq!(DmPolicy::from_str("unknown"), DmPolicy::Open);
    }
}
