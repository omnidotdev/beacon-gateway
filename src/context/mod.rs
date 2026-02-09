//! Context building for AI assistant conversations
//!
//! Combines:
//! - Session history (recent messages)
//! - User context (learned preferences)
//! - life.json data (portable identity)

mod life_json;
mod builder;

pub use builder::{BuiltContext, ContextBuilder, ContextConfig, ContextMessage};
pub use life_json::{LifeJson, LifeJsonReader};
