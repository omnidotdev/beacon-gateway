//! Text embedding for semantic memory search
//!
//! Re-exports from agent-core. Error conversion from `EmbedderError`
//! to Beacon's `Error` is handled via `From` impl in `error.rs`

pub use agent_core::knowledge::{EMBEDDING_DIM, Embedder, EmbedderError};
