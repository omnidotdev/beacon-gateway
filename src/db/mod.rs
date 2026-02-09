//! Database module for session, message, and memory persistence

pub mod embedder;
pub mod indexer;
pub mod memory;
pub mod persona;
mod schema;
pub mod session;
pub mod skill;
pub mod user;

use std::path::Path;
use std::sync::Once;

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;

use crate::{Error, Result};

static SQLITE_VEC_INIT: Once = Once::new();

/// Register sqlite-vec extension for all new connections
///
/// This must be called before creating any database connections.
/// Safe to call multiple times; only the first call has any effect.
#[allow(unsafe_code)]
pub(crate) fn register_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the initialization function provided by the
        // sqlite-vec crate. It is designed to be passed to `sqlite3_auto_extension`.
        // The transmute converts the function pointer to the correct signature
        // expected by `SQLite`'s auto_extension registration API.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

pub use embedder::{Embedder, EMBEDDING_DIM};
pub use indexer::{ExtractedFact, ExtractionResponse, Indexer};
pub use memory::{Memory, MemoryCategory, MemoryRepo};
pub use persona::{InstalledPersona, PersonaRepo};
pub use schema::SCHEMA_VERSION;
pub use session::{Message, MessageRole, Session, SessionRepo};
pub use skill::SkillRepo;
pub use user::{User, UserContext, UserRepo};

/// Database connection pool
pub type DbPool = Pool<SqliteConnectionManager>;

/// Pooled database connection
pub type DbConn = PooledConnection<SqliteConnectionManager>;

/// Initialize the database
///
/// # Errors
///
/// Returns error if database cannot be opened or initialized
pub fn init<P: AsRef<Path>>(path: P) -> Result<DbPool> {
    // Register sqlite-vec before creating any connections
    register_sqlite_vec();

    let manager = SqliteConnectionManager::file(path);
    let pool = Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| Error::Database(e.to_string()))?;

    // Run migrations on first connection
    let conn = pool.get().map_err(|e| Error::Database(e.to_string()))?;
    schema::init(&conn)?;

    tracing::info!(version = SCHEMA_VERSION, "database initialized");
    Ok(pool)
}

/// Initialize an in-memory database (for testing)
///
/// # Errors
///
/// Returns error if database cannot be initialized
pub fn init_memory() -> Result<DbPool> {
    // Register sqlite-vec before creating any connections
    register_sqlite_vec();

    let manager = SqliteConnectionManager::memory();
    let pool = Pool::builder()
        .max_size(1)
        .build(manager)
        .map_err(|e| Error::Database(e.to_string()))?;

    let conn = pool.get().map_err(|e| Error::Database(e.to_string()))?;
    schema::init(&conn)?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_memory() {
        let pool = init_memory().unwrap();
        let _conn = pool.get().unwrap();
    }
}
