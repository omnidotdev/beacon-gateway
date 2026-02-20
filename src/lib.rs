//! Beacon Gateway - Voice and messaging gateway for AI assistants
//!
//! This library provides the core functionality for the Beacon gateway:
//! - Voice processing (wake word detection, STT, TTS)
//! - Messaging channel adapters
//! - Persona management
//! - LLM routing via Synapse AI router
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                    Interfaces                        │
//! │   Voice  │  Discord  │  Slack  │  WhatsApp  │  ...  │
//! └────────────────────┬────────────────────────────────┘
//!                      │
//! ┌────────────────────▼────────────────────────────────┐
//! │                 Beacon Gateway                       │
//! │   Daemon  │  Wake Word  │  STT/TTS  │  Channels    │
//! └────────────────────┬────────────────────────────────┘
//!                      │
//! ┌────────────────────▼────────────────────────────────┐
//! │            Synapse (AI Router)                       │
//! │   LLM  │  MCP  │  STT  │  TTS                      │
//! └─────────────────────────────────────────────────────┘
//! ```

pub mod agent;
pub mod api;
pub mod billing;
pub mod attachments;
pub mod events;
pub mod canvas;
pub mod channels;
pub mod config;
pub mod context;
pub mod daemon;
pub mod db;
pub mod discovery;
pub mod error;
pub mod hooks;
pub mod extensions;
pub mod integrations;
pub mod knowledge;
pub mod lifecycle;
pub mod links;
pub mod media;
pub mod nodes;
pub mod persona;
pub mod plugins;
pub mod providers;
pub mod relay;
pub mod security;
pub mod skills;
pub mod sync;
pub mod tools;
pub mod voice;

/// Sentinel persona ID indicating no persona should be applied
pub const NO_PERSONA_ID: &str = "__none__";

pub use config::Config;
pub use context::{ContextBuilder, LifeJson, LifeJsonReader};
pub use daemon::Daemon;
pub use db::{DbConn, DbPool};
pub use error::{Error, Result};
pub use knowledge::{
    KnowledgePackResolver, ResolverError, cosine_similarity, format_knowledge,
    hydrate_embeddings, select_knowledge, select_knowledge_with_embeddings,
};
pub use persona::{
    KnowledgeChunk, KnowledgeConfig, KnowledgePack, KnowledgePackRef, KnowledgePriority,
    PackEmbeddings, Persona,
};
pub use providers::KeyResolver;
pub use security::{DmPolicy, PairedUser, PairingManager};
pub use skills::{Skill, SkillMetadata, SkillRegistry, SkillSource};
pub use tools::{
    SearchProvider, SearchResult, ToolPolicy, ToolPolicyConfig, ToolProfile, WebFetchTool,
    WebResponse, WebSearchTool,
};
pub use canvas::{Canvas, CanvasCommand, CanvasContent, CanvasElement, CanvasTools};
pub use extensions::{Extension, ExtensionInfo, ExtensionRegistry};
pub use integrations::{Schedule, ScheduleRequest, VortexClient};
pub use relay::{RelayConfig, RelayManager, RelayMode, RelayStatus};
pub use discovery::MdnsAdvertiser;
pub use hooks::{HookAction, HookEvent, HookManager, HookResult, HooksConfig};
pub use plugins::{PluginKind, PluginManifest, PluginManager};
pub use sync::SyncClient;
