//! HTTP API server for beacon gateway

pub mod admin;
pub mod feedback;
pub use feedback::{FeedbackAnswer, FeedbackManager};
mod auth;
pub mod browser;
pub mod canvas;
pub mod health;
pub mod jwt;
pub mod knowledge;
pub mod life_json;
pub mod nodes;
pub mod pairing;
pub mod personas;
pub mod plugins;
pub mod providers;
pub mod rate_limit;
pub mod skills;
pub mod voice;
pub mod webhooks;
pub mod websocket;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use synapse_client::SynapseClient;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock}; // Mutex still used for Canvas
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::canvas::Canvas;
use crate::channels::{TeamsChannel, TelegramChannel};
use crate::context::ContextConfig;
use crate::db::{DbPool, Embedder, Indexer, MemoryRepo, SessionRepo, SkillRepo, UserRepo};
use crate::nodes::NodeRegistry;
use crate::tools::ToolPolicy;
use crate::Result;

/// Information about the current LLM model
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub provider: String,
}

/// Dynamic persona state that can be updated at runtime
#[derive(Debug)]
pub struct ActivePersona {
    pub id: String,
    pub system_prompt: Option<String>,
}

/// Shared state for API handlers
#[derive(Clone)]
pub struct ApiState {
    pub db: DbPool,
    pub api_key: Option<String>,
    pub persona_id: String,
    pub persona_system_prompt: Option<String>,
    pub active_persona: Arc<RwLock<ActivePersona>>,
    pub persona_cache_dir: PathBuf,
    pub synapse: Option<Arc<SynapseClient>>,
    pub llm_model: String,
    pub llm_max_tokens: u32,
    pub system_prompt: String,
    pub telegram: Option<TelegramChannel>,
    pub teams: Option<TeamsChannel>,
    pub session_repo: SessionRepo,
    pub user_repo: UserRepo,
    pub memory_repo: MemoryRepo,
    /// Text embedder for semantic memory search.
    /// Present only when `OPENAI_API_KEY` is set.
    pub embedder: Option<Arc<Embedder>>,
    /// Conversation indexer for post-turn memory extraction.
    /// Present only when `OPENAI_API_KEY` is set.
    pub indexer: Option<Arc<Indexer>>,
    pub skill_repo: SkillRepo,
    pub tool_policy: Arc<ToolPolicy>,
    pub manifold_url: String,
    pub stt_model: String,
    pub tts_model: String,
    pub tts_voice: String,
    pub tts_speed: f64,
    pub model_info: Option<ModelInfo>,
    pub browser: browser::SharedBrowser,
    pub canvas: Arc<Mutex<Canvas>>,
    pub node_registry: nodes::SharedNodeRegistry,
    pub plugin_manager: plugins::SharedPluginManager,
    pub key_resolver: Option<Arc<crate::providers::KeyResolver>>,
    pub key_provisioner: Option<Arc<crate::providers::KeyProvisioner>>,
    pub jwt_cache: Option<Arc<jwt::JwksCache>>,
    pub persona_knowledge: Vec<crate::persona::KnowledgeChunk>,
    pub max_context_tokens: usize,
    pub knowledge_cache_dir: PathBuf,
    pub cloud_mode: bool,
    pub rate_limiter: Option<rate_limit::SharedLimiter>,
}

/// Configuration for building an API server
pub struct ApiServerBuilder {
    db: DbPool,
    api_key: Option<String>,
    persona_id: String,
    persona_system_prompt: Option<String>,
    persona_cache_dir: PathBuf,
    port: u16,
    synapse: Option<Arc<SynapseClient>>,
    llm_model: String,
    llm_max_tokens: u32,
    system_prompt: String,
    telegram: Option<TelegramChannel>,
    teams: Option<TeamsChannel>,
    tool_policy: Arc<ToolPolicy>,
    manifold_url: Option<String>,
    static_dir: Option<PathBuf>,
    stt_model: String,
    tts_model: String,
    tts_voice: String,
    tts_speed: f64,
    model_info: Option<ModelInfo>,
    key_resolver: Option<Arc<crate::providers::KeyResolver>>,
    key_provisioner: Option<Arc<crate::providers::KeyProvisioner>>,
    jwt_cache: Option<Arc<jwt::JwksCache>>,
    persona_knowledge: Vec<crate::persona::KnowledgeChunk>,
    max_context_tokens: usize,
    knowledge_cache_dir: Option<PathBuf>,
    plugin_manager: Option<plugins::SharedPluginManager>,
    cloud_mode: bool,
}

impl ApiServerBuilder {
    /// Create a new API server builder
    #[must_use]
    pub fn new(
        db: DbPool,
        persona_id: String,
        persona_system_prompt: Option<String>,
        persona_cache_dir: PathBuf,
        port: u16,
        tool_policy: Arc<ToolPolicy>,
    ) -> Self {
        Self {
            db,
            api_key: None,
            persona_id,
            persona_system_prompt,
            persona_cache_dir,
            port,
            synapse: None,
            llm_model: crate::daemon::DEFAULT_MODEL.to_string(),
            llm_max_tokens: 1024,
            system_prompt: String::new(),
            telegram: None,
            teams: None,
            tool_policy,
            manifold_url: None,
            static_dir: None,
            stt_model: "whisper-1".to_string(),
            tts_model: "tts-1".to_string(),
            tts_voice: "alloy".to_string(),
            tts_speed: 1.0,
            model_info: None,
            key_resolver: None,
            key_provisioner: None,
            jwt_cache: None,
            persona_knowledge: Vec::new(),
            max_context_tokens: 8000,
            knowledge_cache_dir: None,
            plugin_manager: None,
            cloud_mode: false,
        }
    }

    /// Set the API key for admin endpoints
    #[must_use]
    pub fn api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key;
        self
    }

    /// Set the Synapse client for chat
    #[must_use]
    pub fn synapse(mut self, client: Arc<SynapseClient>) -> Self {
        self.synapse = Some(client);
        self
    }

    /// Set the LLM model identifier
    #[must_use]
    pub fn llm_model(mut self, model: String) -> Self {
        self.llm_model = model;
        self
    }

    /// Set the max tokens for LLM responses
    #[must_use]
    pub fn llm_max_tokens(mut self, tokens: u32) -> Self {
        self.llm_max_tokens = tokens;
        self
    }

    /// Set the system prompt for chat
    #[must_use]
    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Set the Telegram channel
    #[must_use]
    pub fn telegram(mut self, channel: TelegramChannel) -> Self {
        self.telegram = Some(channel);
        self
    }

    /// Set the Teams channel
    #[must_use]
    pub fn teams(mut self, channel: TeamsChannel) -> Self {
        self.teams = Some(channel);
        self
    }

    /// Set the Manifold URL
    #[must_use]
    pub fn manifold_url(mut self, url: Option<String>) -> Self {
        self.manifold_url = url;
        self
    }

    /// Set the static files directory for serving the web UI
    #[must_use]
    pub fn static_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.static_dir = dir;
        self
    }

    /// Set voice configuration from `VoiceConfig`
    #[must_use]
    pub fn voice_config(mut self, config: &crate::config::VoiceConfig) -> Self {
        self.stt_model = config.stt_model.clone();
        self.tts_model = config.tts_model.clone();
        self.tts_voice = config.tts_voice.clone();
        self.tts_speed = config.tts_speed;
        self
    }

    /// Set the model info
    #[must_use]
    pub fn model_info(mut self, info: ModelInfo) -> Self {
        self.model_info = Some(info);
        self
    }

    /// Set the key resolver for BYOK
    #[must_use]
    pub fn key_resolver(mut self, resolver: Arc<crate::providers::KeyResolver>) -> Self {
        self.key_resolver = Some(resolver);
        self
    }

    /// Set the key provisioner for auto-provisioning managed keys
    #[must_use]
    pub fn key_provisioner(mut self, provisioner: Arc<crate::providers::KeyProvisioner>) -> Self {
        self.key_provisioner = Some(provisioner);
        self
    }

    /// Set the JWT cache for Gatekeeper token validation
    #[must_use]
    pub fn jwt_cache(mut self, cache: Arc<jwt::JwksCache>) -> Self {
        self.jwt_cache = Some(cache);
        self
    }

    /// Set persona knowledge chunks for context injection
    #[must_use]
    pub fn persona_knowledge(mut self, chunks: Vec<crate::persona::KnowledgeChunk>) -> Self {
        self.persona_knowledge = chunks;
        self
    }

    /// Set maximum context tokens from persona config
    #[must_use]
    pub fn max_context_tokens(mut self, tokens: usize) -> Self {
        self.max_context_tokens = tokens;
        self
    }

    /// Set the knowledge pack cache directory
    #[must_use]
    pub fn knowledge_cache_dir(mut self, dir: PathBuf) -> Self {
        self.knowledge_cache_dir = Some(dir);
        self
    }

    /// Enable cloud mode (requires JWT, enables rate limiting)
    #[must_use]
    pub fn cloud_mode(mut self, enabled: bool) -> Self {
        self.cloud_mode = enabled;
        self
    }

    /// Set a pre-built plugin manager (shared with daemon)
    #[must_use]
    pub fn plugin_manager(mut self, pm: plugins::SharedPluginManager) -> Self {
        self.plugin_manager = Some(pm);
        self
    }

    /// Build the API server
    #[must_use]
    pub fn build(self) -> ApiServer {
        let session_repo = SessionRepo::new(self.db.clone());
        let user_repo = UserRepo::new(self.db.clone());
        let memory_repo = MemoryRepo::new(self.db.clone());
        let skill_repo = SkillRepo::new(self.db.clone());

        // Create embedder and indexer if OPENAI_API_KEY is set
        let embedder = std::env::var("OPENAI_API_KEY")
            .ok()
            .and_then(|key| Embedder::new(key).ok())
            .map(Arc::new);

        let indexer = embedder.as_ref().and_then(|emb| {
            std::env::var("OPENAI_API_KEY").ok().map(|key| {
                Arc::new(Indexer::new((**emb).clone(), memory_repo.clone(), key))
            })
        });
        let manifold_url = self
            .manifold_url
            .unwrap_or_else(|| "https://api.manifold.omni.dev".to_string());

        let browser = browser::default_browser();
        let canvas = Arc::new(Mutex::new(Canvas::new()));
        let node_registry = Arc::new(Mutex::new(NodeRegistry::new()));
        let plugin_manager = self.plugin_manager.unwrap_or_else(|| {
            let mut pm = crate::plugins::PluginManager::new();
            let dirs = crate::plugins::default_plugin_dirs();
            let loaded = pm.load_all(&dirs);
            if !loaded.is_empty() {
                tracing::info!(count = loaded.len(), plugins = ?loaded, "loaded plugins");
            }
            Arc::new(Mutex::new(pm))
        });

        let active_persona = Arc::new(RwLock::new(ActivePersona {
            id: self.persona_id.clone(),
            system_prompt: self.persona_system_prompt.clone(),
        }));

        let rate_limiter = if self.cloud_mode {
            Some(rate_limit::create_limiter(120))
        } else {
            None
        };

        let state = Arc::new(ApiState {
            db: self.db,
            api_key: self.api_key,
            persona_id: self.persona_id,
            persona_system_prompt: self.persona_system_prompt,
            active_persona,
            persona_cache_dir: self.persona_cache_dir,
            synapse: self.synapse,
            llm_model: self.llm_model,
            llm_max_tokens: self.llm_max_tokens,
            system_prompt: self.system_prompt,
            telegram: self.telegram,
            teams: self.teams,
            session_repo,
            user_repo,
            memory_repo,
            embedder,
            indexer,
            skill_repo,
            tool_policy: self.tool_policy,
            manifold_url,
            stt_model: self.stt_model,
            tts_model: self.tts_model,
            tts_voice: self.tts_voice,
            tts_speed: self.tts_speed,
            model_info: self.model_info,
            browser,
            canvas,
            node_registry,
            plugin_manager,
            key_resolver: self.key_resolver,
            key_provisioner: self.key_provisioner,
            jwt_cache: self.jwt_cache,
            persona_knowledge: self.persona_knowledge,
            max_context_tokens: self.max_context_tokens,
            knowledge_cache_dir: self.knowledge_cache_dir.unwrap_or_else(|| {
                PathBuf::from(".cache/omni/knowledge")
            }),
            cloud_mode: self.cloud_mode,
            rate_limiter,
        });

        ApiServer {
            state,
            port: self.port,
            static_dir: self.static_dir,
        }
    }
}

/// API server
pub struct ApiServer {
    state: Arc<ApiState>,
    port: u16,
    static_dir: Option<PathBuf>,
}

impl ApiServer {
    /// Build context configuration
    #[must_use]
    pub fn context_config(persona_id: &str, persona_system_prompt: Option<String>) -> ContextConfig {
        ContextConfig {
            max_messages: 20,
            max_tokens: 4000,
            persona_id: persona_id.to_string(),
            max_memories: 10,
            persona_system_prompt,
        }
    }

    /// Build the router with all routes
    fn router(&self) -> Router {
        let mut router = Router::new()
            .nest("/api/admin", admin::router(self.state.clone()))
            .nest("/api/canvas", canvas::api::router(self.state.canvas.clone()))
            .nest("/api/providers", providers::router(self.state.clone()))
            .nest("/api/knowledge", knowledge::router(self.state.clone()))
            .nest("/api/memories", life_json::router(self.state.clone()))
            .nest("/api/skills", skills::router(self.state.clone()))
            .nest("/api/personas/marketplace", personas::router(self.state.clone()))
            .nest("/api/voice", voice::router(self.state.clone()))
            .nest("/api/webhooks", webhooks::router(self.state.clone()))
            .nest("/api/browser", browser::router(self.state.browser.clone()))
            .nest("/api/nodes", nodes::router(self.state.node_registry.clone()))
            .nest("/api/plugins", plugins::router(self.state.plugin_manager.clone()))
            .nest("/ws", websocket::router(self.state.clone()))
            .nest("/ws", nodes::ws_router(self.state.node_registry.clone()))
            .nest("/ws/canvas", canvas::router(self.state.canvas.clone()))
            .merge(health::router())
            .merge(health::ready_router(self.state.clone()));

        // Serve static files if configured
        if let Some(static_dir) = &self.static_dir {
            let index_file = static_dir.join("index.html");
            let serve_dir = ServeDir::new(static_dir)
                .not_found_service(ServeFile::new(&index_file));

            router = router.fallback_service(serve_dir);
            tracing::info!(path = %static_dir.display(), "serving static files");
        }

        // Rate limiting (cloud mode only)
        let router = router.layer(axum::middleware::from_fn_with_state(
            self.state.clone(),
            rate_limit::rate_limit_middleware,
        ));

        // CORS layer for cross-origin requests from frontend
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        router.layer(cors).layer(TraceLayer::new_for_http())
    }

    /// Run the API server
    ///
    /// # Errors
    ///
    /// Returns error if server fails to bind or run
    pub async fn run(self) -> Result<()> {
        if self.state.cloud_mode {
            tracing::info!("cloud mode enabled: JWT required, rate limiting active");
            if self.state.synapse.is_none() {
                tracing::error!("cloud mode enabled but no Synapse configured - users without BYOK keys will get errors");
            }
        }

        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| crate::Error::Config(format!("failed to bind API server: {e}")))?;

        tracing::info!(port = self.port, "API server listening");

        axum::serve(listener, self.router())
            .await
            .map_err(|e| crate::Error::Config(format!("API server error: {e}")))?;

        Ok(())
    }

    /// Run the API server in a background task
    #[must_use]
    pub fn spawn(self) -> tokio::task::JoinHandle<Result<()>> {
        tokio::spawn(async move { self.run().await })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_state_has_embedder_and_indexer_fields() {
        // Compile-time assertion: these fields must exist on ApiState
        fn _check(state: &crate::api::ApiState) {
            let _: &Option<std::sync::Arc<crate::db::Embedder>> = &state.embedder;
            let _: &Option<std::sync::Arc<crate::db::Indexer>> = &state.indexer;
        }
    }
}
