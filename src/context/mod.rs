//! Context building for AI assistant conversations
//!
//! Combines:
//! - Session history (recent messages)
//! - User context (learned preferences)
//! - life.json data (portable identity)

mod builder;
pub mod compaction;
mod life_json;
pub mod life_json_sync;

pub use builder::{BuiltContext, ContextBuilder, ContextConfig, ContextMessage};
pub use compaction::{CompactionConfig, CompactionResult, SessionCompactor};
pub use life_json::{LifeJson, LifeJsonReader};
pub use life_json_sync::{ExportResult, ImportResult};
