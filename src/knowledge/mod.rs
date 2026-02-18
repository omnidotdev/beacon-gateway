//! Knowledge management for persona context
//!
//! - **selection**: Choose relevant knowledge chunks based on user messages
//! - **resolver**: Fetch and cache knowledge packs from Manifold

mod resolver;
mod selection;

pub use resolver::{KnowledgePackResolver, ResolverError, hydrate_embeddings};
pub use selection::{
    cosine_similarity, format_knowledge, select_knowledge, select_knowledge_with_embeddings,
};
