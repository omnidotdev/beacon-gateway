//! Beacon Gateway - Voice and messaging gateway for AI assistants
//!
//! This library provides the core functionality for the Beacon gateway:
//! - Voice processing (wake word detection, STT, TTS)
//! - Messaging channel adapters
//! - Persona management
//! - Agent integration via Omni CLI
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
//! │              Omni CLI (Agent Core)                   │
//! └─────────────────────────────────────────────────────┘
//! ```

pub mod api;
pub mod attachments;
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
pub mod links;
pub mod media;
pub mod persona;
pub mod providers;
pub mod relay;
pub mod security;
pub mod skills;
pub mod tools;
pub mod voice;

pub use config::Config;
pub use context::{ContextBuilder, LifeJson, LifeJsonReader};
pub use daemon::Daemon;
pub use db::{DbConn, DbPool};
pub use error::{Error, Result};
pub use knowledge::{format_knowledge, select_knowledge};
pub use persona::{
    KnowledgeChunk, KnowledgeConfig, KnowledgePack, KnowledgePackRef, KnowledgePriority, Persona,
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
