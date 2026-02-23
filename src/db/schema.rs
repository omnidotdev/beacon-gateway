//! Database schema and migrations

use rusqlite::Connection;

use crate::Result;

/// Current schema version
pub const SCHEMA_VERSION: i32 = 16;

/// Initialize the database schema
///
/// # Errors
///
/// Returns error if migration fails
pub fn init(conn: &Connection) -> Result<()> {
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);

    if version < 1 {
        migrate_v1(conn)?;
    }
    if version < 2 {
        migrate_v2(conn)?;
    }
    if version < 3 {
        migrate_v3(conn)?;
    }
    if version < 4 {
        migrate_v4(conn)?;
    }
    if version < 5 {
        migrate_v5(conn)?;
    }
    if version < 6 {
        migrate_v6(conn)?;
    }
    if version < 7 {
        migrate_v7(conn)?;
    }
    if version < 8 {
        migrate_v8(conn)?;
    }
    if version < 9 {
        migrate_v9(conn)?;
    }
    if version < 10 {
        migrate_v10(conn)?;
    }
    if version < 11 {
        migrate_v11(conn)?;
    }
    if version < 12 {
        migrate_v12(conn)?;
    }
    if version < 13 {
        migrate_v13(conn)?;
    }
    if version < 14 {
        migrate_v14(conn)?;
    }
    if version < 15 {
        migrate_v15(conn)?;
    }
    if version < 16 {
        migrate_v16(conn)?;
    }

    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Users table
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            life_json_path TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Sessions table
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            channel TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            persona_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel, channel_id);

        -- Messages table
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id),
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);

        -- User context (learned preferences)
        CREATE TABLE IF NOT EXISTS user_context (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'learned',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, key)
        );

        CREATE INDEX IF NOT EXISTS idx_user_context_user ON user_context(user_id);

        PRAGMA user_version = 1;
        ",
    )?;

    tracing::info!("migrated to schema v1");
    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Memories table for long-term memory storage
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            category TEXT NOT NULL CHECK(category IN ('preference', 'fact', 'correction', 'general')),
            content TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '[]',
            pinned INTEGER NOT NULL DEFAULT 0,
            access_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            accessed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
        CREATE INDEX IF NOT EXISTS idx_memories_pinned ON memories(pinned);

        PRAGMA user_version = 2;
        ",
    )?;

    tracing::info!("migrated to schema v2");
    Ok(())
}

fn migrate_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Installed skills table
        CREATE TABLE IF NOT EXISTS installed_skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            version TEXT,
            author TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            permissions TEXT NOT NULL DEFAULT '[]',
            content TEXT NOT NULL,
            source_type TEXT NOT NULL CHECK(source_type IN ('local', 'manifold', 'bundled', 'plugin')),
            source_namespace TEXT,
            source_repository TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_skills_name ON installed_skills(name);
        CREATE INDEX IF NOT EXISTS idx_skills_enabled ON installed_skills(enabled);
        CREATE INDEX IF NOT EXISTS idx_skills_source ON installed_skills(source_type);

        PRAGMA user_version = 3;
        ",
    )?;

    tracing::info!("migrated to schema v3");
    Ok(())
}

fn migrate_v4(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Installed personas table (from marketplace)
        CREATE TABLE IF NOT EXISTS installed_personas (
            id TEXT PRIMARY KEY,
            persona_id TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            tagline TEXT,
            avatar TEXT,
            accent_color TEXT,
            content TEXT NOT NULL,
            source_namespace TEXT NOT NULL,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_personas_persona_id ON installed_personas(persona_id);
        CREATE INDEX IF NOT EXISTS idx_personas_namespace ON installed_personas(source_namespace);

        PRAGMA user_version = 4;
        ",
    )?;

    tracing::info!("migrated to schema v4");
    Ok(())
}

fn migrate_v5(conn: &Connection) -> Result<()> {
    // Note: sqlite-vec extension is registered globally in db::init()
    // before any connections are created

    conn.execute_batch(
        r"
        -- Add embedding column to memories
        ALTER TABLE memories ADD COLUMN embedding BLOB;

        -- Add source metadata columns
        ALTER TABLE memories ADD COLUMN source_session_id TEXT;
        ALTER TABLE memories ADD COLUMN source_channel TEXT;

        -- Create virtual table for vector search
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
            memory_id TEXT PRIMARY KEY,
            embedding FLOAT[1536]
        );

        PRAGMA user_version = 5;
        ",
    )?;

    tracing::info!("migrated to schema v5 (vector search)");
    Ok(())
}

fn migrate_v6(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Paired users table for DM security
        CREATE TABLE IF NOT EXISTS paired_users (
            id TEXT PRIMARY KEY,
            sender_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            paired_at TEXT NOT NULL,
            pairing_code TEXT,
            code_expires_at TEXT,
            UNIQUE(sender_id, channel)
        );

        CREATE INDEX IF NOT EXISTS idx_paired_users_sender ON paired_users(sender_id, channel);
        CREATE INDEX IF NOT EXISTS idx_paired_users_channel ON paired_users(channel);

        PRAGMA user_version = 6;
        ",
    )?;

    tracing::info!("migrated to schema v6 (DM pairing)");
    Ok(())
}

fn migrate_v7(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Paired devices table for device identity system
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            public_key BLOB NOT NULL UNIQUE,
            name TEXT NOT NULL,
            platform TEXT,
            trust_level TEXT NOT NULL DEFAULT 'paired',
            paired_at TEXT NOT NULL,
            last_seen TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_devices_public_key ON devices(public_key);
        CREATE INDEX IF NOT EXISTS idx_devices_last_seen ON devices(last_seen);

        PRAGMA user_version = 7;
        ",
    )?;

    tracing::info!("migrated to schema v7 (device identity)");
    Ok(())
}

fn migrate_v8(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Add thread_id to messages for conversation threading
        ALTER TABLE messages ADD COLUMN thread_id TEXT;

        -- Index for efficient thread queries
        CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(session_id, thread_id);

        PRAGMA user_version = 8;
        ",
    )?;

    tracing::info!("migrated to schema v8 (message threading)");
    Ok(())
}

fn migrate_v9(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Add sync metadata columns to memories
        ALTER TABLE memories ADD COLUMN content_hash TEXT;
        ALTER TABLE memories ADD COLUMN origin_device_id TEXT;
        ALTER TABLE memories ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'));
        ALTER TABLE memories ADD COLUMN deleted_at TEXT;
        ALTER TABLE memories ADD COLUMN synced_at TEXT;
        ALTER TABLE memories ADD COLUMN cloud_id TEXT;

        CREATE INDEX IF NOT EXISTS idx_memories_synced ON memories(synced_at);
        CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at);

        PRAGMA user_version = 9;
        ",
    )?;

    // Backfill content_hash for existing memories
    backfill_content_hashes(conn)?;

    tracing::info!("migrated to schema v9 (memory sync)");
    Ok(())
}

/// Backfill content_hash for existing memories that lack one
fn backfill_content_hashes(conn: &Connection) -> Result<()> {
    use sha2::{Digest, Sha256};

    let mut stmt = conn.prepare(
        "SELECT id, content FROM memories WHERE content_hash IS NULL",
    )?;

    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .flatten()
        .collect();

    let count = rows.len();
    for (id, content) in rows {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = hex::encode(hasher.finalize());

        conn.execute(
            "UPDATE memories SET content_hash = ?1 WHERE id = ?2",
            rusqlite::params![hash, id],
        )?;
    }

    if count > 0 {
        tracing::info!(count, "backfilled content hashes for existing memories");
    }

    Ok(())
}

fn migrate_v10(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Installed knowledge packs table
        CREATE TABLE IF NOT EXISTS installed_knowledge_packs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            source_namespace TEXT NOT NULL,
            description TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            chunk_count INTEGER NOT NULL DEFAULT 0,
            has_embeddings INTEGER NOT NULL DEFAULT 0,
            content TEXT NOT NULL,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, source_namespace)
        );

        -- Vector table for knowledge chunk embeddings
        CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_vec USING vec0(
            chunk_id TEXT PRIMARY KEY,
            embedding FLOAT[1536]
        );

        PRAGMA user_version = 10;
        ",
    )?;

    tracing::info!("migrated to schema v10 (knowledge packs)");
    Ok(())
}

fn migrate_v11(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Gateway-local provider keys for self-hosted deployments
        CREATE TABLE IF NOT EXISTS local_provider_keys (
            provider TEXT PRIMARY KEY,
            api_key TEXT NOT NULL,
            model_preference TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        PRAGMA user_version = 11;
        ",
    )?;

    tracing::info!("migrated to schema v11 (local provider keys)");
    Ok(())
}

fn migrate_v12(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Add priority column to installed_skills for prompt hierarchy
        ALTER TABLE installed_skills ADD COLUMN priority TEXT NOT NULL DEFAULT 'standard';

        CREATE INDEX IF NOT EXISTS idx_skills_priority ON installed_skills(priority);

        PRAGMA user_version = 12;
        ",
    )?;

    tracing::info!("migrated to schema v12 (skill priority)");
    Ok(())
}

fn migrate_v13(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Extended skill metadata and per-user scoping
        ALTER TABLE installed_skills ADD COLUMN always_include INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE installed_skills ADD COLUMN user_invocable INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE installed_skills ADD COLUMN disable_model_invocation INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE installed_skills ADD COLUMN emoji TEXT;
        ALTER TABLE installed_skills ADD COLUMN requires_env TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN command_name TEXT;
        ALTER TABLE installed_skills ADD COLUMN user_id TEXT;

        CREATE INDEX IF NOT EXISTS idx_skills_command ON installed_skills(command_name);
        CREATE INDEX IF NOT EXISTS idx_skills_user ON installed_skills(user_id);

        PRAGMA user_version = 13;
        ",
    )?;

    tracing::info!("migrated to schema v13 (extended skill metadata + per-user scoping)");
    Ok(())
}

fn migrate_v14(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- OS, binary, dispatch, and env fields for skills
        ALTER TABLE installed_skills ADD COLUMN os TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN requires_bins TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN requires_any_bins TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN primary_env TEXT;
        ALTER TABLE installed_skills ADD COLUMN command_dispatch_tool TEXT;
        ALTER TABLE installed_skills ADD COLUMN api_key TEXT;
        ALTER TABLE installed_skills ADD COLUMN skill_env TEXT NOT NULL DEFAULT '{}';

        PRAGMA user_version = 14;
        ",
    )?;

    tracing::info!("migrated to schema v14 (OS, bins, dispatch, per-skill env)");
    Ok(())
}

fn migrate_v15(conn: &Connection) -> Result<()> {
    // Recreate table to update CHECK constraint (add 'plugin' source_type)
    // and add install_specs column
    conn.execute_batch(
        r"
        -- Recreate installed_skills with updated source_type CHECK and new column
        CREATE TABLE IF NOT EXISTS installed_skills_new (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            version TEXT,
            author TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            permissions TEXT NOT NULL DEFAULT '[]',
            content TEXT NOT NULL,
            source_type TEXT NOT NULL CHECK(source_type IN ('local', 'manifold', 'bundled', 'plugin')),
            source_namespace TEXT,
            source_repository TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            priority TEXT NOT NULL DEFAULT 'standard',
            always_include INTEGER NOT NULL DEFAULT 0,
            user_invocable INTEGER NOT NULL DEFAULT 1,
            disable_model_invocation INTEGER NOT NULL DEFAULT 0,
            emoji TEXT,
            requires_env TEXT NOT NULL DEFAULT '[]',
            command_name TEXT,
            user_id TEXT,
            os TEXT NOT NULL DEFAULT '[]',
            requires_bins TEXT NOT NULL DEFAULT '[]',
            requires_any_bins TEXT NOT NULL DEFAULT '[]',
            primary_env TEXT,
            command_dispatch_tool TEXT,
            api_key TEXT,
            skill_env TEXT NOT NULL DEFAULT '{}',
            install_specs TEXT NOT NULL DEFAULT '[]'
        );

        INSERT OR IGNORE INTO installed_skills_new
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at, COALESCE(updated_at, datetime('now')),
                   COALESCE(priority, 'standard'),
                   COALESCE(always_include, 0), COALESCE(user_invocable, 1),
                   COALESCE(disable_model_invocation, 0), emoji,
                   COALESCE(requires_env, '[]'), command_name, user_id,
                   COALESCE(os, '[]'), COALESCE(requires_bins, '[]'),
                   COALESCE(requires_any_bins, '[]'), primary_env,
                   command_dispatch_tool, api_key,
                   COALESCE(skill_env, '{}'), '[]'
            FROM installed_skills;

        DROP TABLE installed_skills;
        ALTER TABLE installed_skills_new RENAME TO installed_skills;

        -- Re-create indices
        CREATE INDEX IF NOT EXISTS idx_skills_name ON installed_skills(name);
        CREATE INDEX IF NOT EXISTS idx_skills_enabled ON installed_skills(enabled);
        CREATE INDEX IF NOT EXISTS idx_skills_source ON installed_skills(source_type);
        CREATE INDEX IF NOT EXISTS idx_skills_priority ON installed_skills(priority);
        CREATE INDEX IF NOT EXISTS idx_skills_command ON installed_skills(command_name);
        CREATE INDEX IF NOT EXISTS idx_skills_user ON installed_skills(user_id);

        PRAGMA user_version = 15;
        ",
    )?;

    tracing::info!("migrated to schema v15 (skill install specs, plugin source type)");
    Ok(())
}

fn migrate_v16(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r"
        -- Config-based eligibility paths
        ALTER TABLE installed_skills ADD COLUMN requires_config TEXT NOT NULL DEFAULT '[]';

        PRAGMA user_version = 16;
        ",
    )?;

    tracing::info!("migrated to schema v16 (config-based eligibility)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_conn() -> Connection {
        // Must register sqlite-vec before opening connections
        crate::db::register_sqlite_vec();
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn test_schema_init() {
        let conn = setup_test_conn();
        init(&conn).unwrap();

        // Verify tables exist
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_schema_idempotent() {
        let conn = setup_test_conn();
        init(&conn).unwrap();
        init(&conn).unwrap(); // Should not fail
    }

    #[test]
    fn test_sqlite_vec_loaded() {
        let conn = setup_test_conn();
        init(&conn).unwrap();

        // Verify sqlite-vec is loaded
        let version: String = conn
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .unwrap();
        assert!(version.starts_with('v'));
    }
}
