//! Knowledge management for persona context
//!
//! Re-exports shared infrastructure from agent-core

pub use agent_core::knowledge::{
    KnowledgePackResolver, ResolverError, build_knowledge_context, build_retrieval_query,
    cosine_similarity, format_knowledge, hydrate_embeddings, resolve_and_merge, select_knowledge,
    select_knowledge_with_embeddings,
};
