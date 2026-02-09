//! Paired device management
//!
//! Manages devices that have been paired with this gateway instance

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::DbPool;
use crate::{Error, Result};

/// Trust level for paired devices
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// Standard paired device
    #[default]
    Paired,

    /// Trusted device with elevated permissions
    Trusted,

    /// Admin device with full control
    Admin,
}

impl TrustLevel {
    /// Parse from string representation
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "trusted" => Self::Trusted,
            "admin" => Self::Admin,
            _ => Self::Paired,
        }
    }
}

impl std::fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paired => write!(f, "paired"),
            Self::Trusted => write!(f, "trusted"),
            Self::Admin => write!(f, "admin"),
        }
    }
}

/// A paired device record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Unique device identifier (from device identity)
    pub id: String,

    /// Ed25519 public key (base64 encoded)
    pub public_key: String,

    /// Human-readable device name
    pub name: String,

    /// Platform identifier (e.g., "linux-x86_64")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    /// Trust level for this device
    pub trust_level: TrustLevel,

    /// When the device was paired
    pub paired_at: DateTime<Utc>,

    /// When the device was last seen
    pub last_seen: DateTime<Utc>,
}

/// Manages paired device storage and operations
#[derive(Clone)]
pub struct DeviceManager {
    pool: DbPool,
}

impl DeviceManager {
    /// Create a new device manager
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Register a new paired device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails or device already exists
    pub fn register(
        &self,
        device_id: &str,
        public_key: &str,
        name: &str,
        platform: Option<&str>,
        trust_level: TrustLevel,
    ) -> Result<PairedDevice> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let now = Utc::now();

        conn.execute(
            "INSERT INTO devices (id, public_key, name, platform, trust_level, paired_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            [
                device_id,
                public_key,
                name,
                platform.unwrap_or(""),
                &trust_level.to_string(),
                &now.to_rfc3339(),
                &now.to_rfc3339(),
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                Error::Auth("device already paired".to_string())
            } else {
                Error::Database(e.to_string())
            }
        })?;

        tracing::info!(device_id, name, "device paired");

        Ok(PairedDevice {
            id: device_id.to_string(),
            public_key: public_key.to_string(),
            name: name.to_string(),
            platform: platform.map(ToString::to_string),
            trust_level,
            paired_at: now,
            last_seen: now,
        })
    }

    /// Get a device by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get(&self, device_id: &str) -> Result<Option<PairedDevice>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            "SELECT id, public_key, name, platform, trust_level, paired_at, last_seen
             FROM devices WHERE id = ?1",
            [device_id],
            |row| {
                Ok(PairedDevice {
                    id: row.get(0)?,
                    public_key: row.get(1)?,
                    name: row.get(2)?,
                    platform: row.get::<_, Option<String>>(3)?,
                    trust_level: TrustLevel::from_str(&row.get::<_, String>(4)?),
                    paired_at: parse_datetime(&row.get::<_, String>(5)?),
                    last_seen: parse_datetime(&row.get::<_, String>(6)?),
                })
            },
        );

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// Get a device by public key
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn get_by_public_key(&self, public_key: &str) -> Result<Option<PairedDevice>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let result = conn.query_row(
            "SELECT id, public_key, name, platform, trust_level, paired_at, last_seen
             FROM devices WHERE public_key = ?1",
            [public_key],
            |row| {
                Ok(PairedDevice {
                    id: row.get(0)?,
                    public_key: row.get(1)?,
                    name: row.get(2)?,
                    platform: row.get::<_, Option<String>>(3)?,
                    trust_level: TrustLevel::from_str(&row.get::<_, String>(4)?),
                    paired_at: parse_datetime(&row.get::<_, String>(5)?),
                    last_seen: parse_datetime(&row.get::<_, String>(6)?),
                })
            },
        );

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// Update last seen timestamp for a device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn update_last_seen(&self, device_id: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE devices SET last_seen = ?1 WHERE id = ?2",
            [&now, device_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }

    /// Update device trust level
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn update_trust_level(&self, device_id: &str, trust_level: TrustLevel) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        conn.execute(
            "UPDATE devices SET trust_level = ?1 WHERE id = ?2",
            [&trust_level.to_string(), device_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        tracing::info!(device_id, %trust_level, "updated device trust level");
        Ok(())
    }

    /// Update device name
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn update_name(&self, device_id: &str, name: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        conn.execute(
            "UPDATE devices SET name = ?1 WHERE id = ?2",
            [name, device_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }

    /// Remove a device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn remove(&self, device_id: &str) -> Result<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let rows = conn
            .execute("DELETE FROM devices WHERE id = ?1", [device_id])
            .map_err(|e| Error::Database(e.to_string()))?;

        if rows > 0 {
            tracing::info!(device_id, "device removed");
        }

        Ok(rows > 0)
    }

    /// List all paired devices
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn list(&self) -> Result<Vec<PairedDevice>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, public_key, name, platform, trust_level, paired_at, last_seen
                 FROM devices ORDER BY last_seen DESC",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let devices = stmt
            .query_map([], |row| {
                Ok(PairedDevice {
                    id: row.get(0)?,
                    public_key: row.get(1)?,
                    name: row.get(2)?,
                    platform: row.get::<_, Option<String>>(3)?,
                    trust_level: TrustLevel::from_str(&row.get::<_, String>(4)?),
                    paired_at: parse_datetime(&row.get::<_, String>(5)?),
                    last_seen: parse_datetime(&row.get::<_, String>(6)?),
                })
            })
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(devices)
    }

    /// Count paired devices
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn count(&self) -> Result<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM devices", [], |row| row.get(0))
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Check if a device is paired (exists in database)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn is_paired(&self, device_id: &str) -> Result<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM devices WHERE id = ?1",
                [device_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        Ok(exists)
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup() -> DeviceManager {
        let pool = init_memory().unwrap();
        DeviceManager::new(pool)
    }

    #[test]
    fn test_register_device() {
        let manager = setup();

        let device = manager
            .register("device123", "public_key_base64", "My Laptop", Some("linux-x86_64"), TrustLevel::Paired)
            .unwrap();

        assert_eq!(device.id, "device123");
        assert_eq!(device.name, "My Laptop");
        assert_eq!(device.trust_level, TrustLevel::Paired);
    }

    #[test]
    fn test_get_device() {
        let manager = setup();

        manager
            .register("device456", "pk456", "Phone", None, TrustLevel::Trusted)
            .unwrap();

        let device = manager.get("device456").unwrap().unwrap();
        assert_eq!(device.name, "Phone");
        assert_eq!(device.trust_level, TrustLevel::Trusted);

        // Non-existent device
        assert!(manager.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_duplicate_device_fails() {
        let manager = setup();

        manager
            .register("device789", "pk789", "Device 1", None, TrustLevel::Paired)
            .unwrap();

        // Same ID should fail
        let result = manager.register("device789", "pk_different", "Device 2", None, TrustLevel::Paired);
        assert!(result.is_err());

        // Same public key should fail
        let result = manager.register("different_id", "pk789", "Device 3", None, TrustLevel::Paired);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_trust_level() {
        let manager = setup();

        manager
            .register("deviceA", "pkA", "Test", None, TrustLevel::Paired)
            .unwrap();

        manager.update_trust_level("deviceA", TrustLevel::Admin).unwrap();

        let device = manager.get("deviceA").unwrap().unwrap();
        assert_eq!(device.trust_level, TrustLevel::Admin);
    }

    #[test]
    fn test_remove_device() {
        let manager = setup();

        manager
            .register("deviceB", "pkB", "Test", None, TrustLevel::Paired)
            .unwrap();

        assert!(manager.is_paired("deviceB").unwrap());
        assert!(manager.remove("deviceB").unwrap());
        assert!(!manager.is_paired("deviceB").unwrap());

        // Removing non-existent device returns false
        assert!(!manager.remove("nonexistent").unwrap());
    }

    #[test]
    fn test_list_devices() {
        let manager = setup();

        manager
            .register("d1", "pk1", "Device 1", None, TrustLevel::Paired)
            .unwrap();
        manager
            .register("d2", "pk2", "Device 2", None, TrustLevel::Trusted)
            .unwrap();
        manager
            .register("d3", "pk3", "Device 3", None, TrustLevel::Admin)
            .unwrap();

        let devices = manager.list().unwrap();
        assert_eq!(devices.len(), 3);
    }

    #[test]
    fn test_count_devices() {
        let manager = setup();

        assert_eq!(manager.count().unwrap(), 0);

        manager
            .register("d1", "pk1", "Device", None, TrustLevel::Paired)
            .unwrap();

        assert_eq!(manager.count().unwrap(), 1);
    }

    #[test]
    fn test_trust_level_parsing() {
        assert_eq!(TrustLevel::from_str("paired"), TrustLevel::Paired);
        assert_eq!(TrustLevel::from_str("trusted"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::from_str("admin"), TrustLevel::Admin);
        assert_eq!(TrustLevel::from_str("unknown"), TrustLevel::Paired);
    }
}
