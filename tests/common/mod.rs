//! Shared test utilities

use beacon_gateway::{DbPool, db};

/// Set up an in-memory test database
#[must_use]
pub fn setup_test_db() -> DbPool {
    db::init_memory().expect("failed to init test db")
}

/// Create a test user in the database
pub fn create_test_user(db: &DbPool, external_id: &str) -> beacon_gateway::db::User {
    let repo = beacon_gateway::db::UserRepo::new(db.clone());
    repo.find_or_create(external_id).expect("failed to create test user")
}

/// Create a test session in the database
pub fn create_test_session(
    db: &DbPool,
    user_id: &str,
    channel: &str,
    channel_id: &str,
    persona_id: &str,
) -> beacon_gateway::db::Session {
    let repo = beacon_gateway::db::SessionRepo::new(db.clone());
    repo.find_or_create(user_id, channel, channel_id, persona_id)
        .expect("failed to create test session")
}
