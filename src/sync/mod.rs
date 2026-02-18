//! Memory sync between gateway and cloud API
//!
//! Each gateway maintains its own SQLite memory store as the authoritative
//! local copy. The cloud API acts as a relay/merge point for cross-device sync.
//! Gateways push deltas up and pull deltas down

pub mod client;
pub mod merge;

pub use client::SyncClient;
pub use merge::merge_memory;
