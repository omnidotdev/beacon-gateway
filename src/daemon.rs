//! Daemon - the main gateway service
//!
//! Orchestrates voice capture, wake word detection, STT, agent, TTS, and messaging channels

use std::sync::Arc;
use std::time::Duration;

use synapse_client::SynapseClient;
use tokio::sync::mpsc;

use crate::api::{ApiServerBuilder, ModelInfo};
use crate::attachments::{AttachmentProcessor, VisionClient};
use futures::StreamExt as _;
use crate::channels::{
    Channel, ChannelCapability, DiscordChannel, GoogleChatChannel, IncomingMessage, MatrixChannel,
    OutgoingMessage, SignalChannel, SlackChannel, TeamsChannel, TelegramChannel, WhatsAppChannel,
};
#[cfg(target_os = "macos")]
use crate::channels::IMessageChannel;
use crate::context::{ContextBuilder, ContextConfig};
use crate::db::{self, DbPool, MessageRole, SessionRepo, SkillRepo, UserRepo};
use crate::security::{DmPolicy, PairingManager};
use crate::voice::{
    AudioCapture, AudioPlayback, SAMPLE_RATE, WakeWordDetector,
    samples_to_wav,
};
use crate::hooks::{HookAction, HookEvent, HookManager};
use crate::{Config, Error, Result};

/// Audio processing chunk size (100ms at 16kHz)
const CHUNK_SIZE: usize = 1600;

/// Default LLM model
pub(crate) const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Max tokens for responses
const MAX_TOKENS: u32 = 1024;

/// The Beacon daemon - orchestrates voice and messaging
pub struct Daemon {
    config: Config,
    port: u16,
    db: DbPool,
}

impl Daemon {
    /// Create a new daemon instance
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails
    #[allow(clippy::unused_async)]
    pub async fn new(config: Config, port: u16) -> Result<Self> {
        let db_path = config.data_dir.join("beacon.db");
        let db = db::init(&db_path)?;

        tracing::info!(path = %db_path.display(), "database initialized");

        Ok(Self { config, port, db })
    }

    /// Get the wake word for this daemon's persona
    #[must_use]
    pub fn wake_word(&self) -> Option<&str> {
        self.config.persona.wake_word()
    }

    /// Initialize the Synapse AI router client
    ///
    /// Returns (client, model_info) - client is None only if the URL is invalid.
    /// In embedded mode (default), runs LLM/STT/TTS in-process.
    /// In cloud mode or without the feature, connects to a remote Synapse server.
    async fn init_synapse(&self) -> (Option<Arc<SynapseClient>>, Option<ModelInfo>) {
        #[cfg(feature = "embedded-synapse")]
        if !self.config.cloud_mode {
            let synapse_cfg =
                crate::config::synapse_bridge::build_synapse_config(&self.config);
            match SynapseClient::embedded(synapse_cfg).await {
                Ok(client) => {
                    let model_id = &self.config.llm_model;
                    tracing::info!(model = %model_id, "embedded synapse initialized");
                    let model_info = ModelInfo {
                        model_id: model_id.clone(),
                        provider: "embedded".to_string(),
                    };
                    return (Some(Arc::new(client)), Some(model_info));
                }
                Err(e) => {
                    tracing::error!(error = %e, "embedded synapse failed, falling back to HTTP");
                }
            }
        }

        match SynapseClient::new(&self.config.synapse_url) {
            Ok(client) => {
                let model_id = &self.config.llm_model;
                tracing::info!(
                    url = %self.config.synapse_url,
                    model = %model_id,
                    "synapse client initialized"
                );

                let model_info = ModelInfo {
                    model_id: model_id.clone(),
                    provider: "synapse".to_string(),
                };

                (Some(Arc::new(client)), Some(model_info))
            }
            Err(e) => {
                tracing::error!(error = %e, url = %self.config.synapse_url, "failed to initialize synapse client");
                tracing::warn!("running in setup mode - chat unavailable");
                (None, None)
            }
        }
    }

    /// Run the daemon until interrupted
    ///
    /// # Errors
    ///
    /// Returns error if the daemon encounters a fatal error
    #[allow(clippy::future_not_send, clippy::too_many_lines)]
    pub async fn run(self) -> Result<()> {
        tracing::info!(
            port = self.port,
            persona = self.config.persona.name(),
            "daemon running"
        );

        // Initialize Iggy event publisher (best-effort; failures do not block startup)
        crate::events::init_publisher(crate::events::EventsConfig::from_env());

        // Initialize cron tools when Vortex is configured
        let cron_tools: Option<Arc<crate::tools::BuiltinCronTools>> =
            if let Some(ref vortex_url) = self.config.api_server.vortex_url {
                tracing::info!(url = %vortex_url, "Vortex scheduling integration available");
                let vortex_api_key = std::env::var("VORTEX_API_KEY").ok();
                let vortex_client = crate::integrations::VortexClient::new(vortex_url, vortex_api_key);
                let callback_url = self
                    .config
                    .api_server
                    .public_url
                    .as_deref()
                    .map_or_else(
                        || format!("http://localhost:{}/api/webhooks/vortex", self.config.api_server.port),
                        |url| format!("{}/api/webhooks/vortex", url.trim_end_matches('/')),
                    );
                Some(Arc::new(crate::tools::BuiltinCronTools::new(
                    crate::tools::CronTools::new(vortex_client, callback_url),
                )))
            } else {
                None
            };

        // Initialize Synapse AI router client
        let (synapse, model_info) = self.init_synapse().await;

        // Get tool policy from persona, applying env var overrides
        let tool_policy = Arc::new(self.config.persona.tool_policy().with_env_overrides());

        // Initialize plugin manager (before skill discovery so plugin skills are included)
        let plugin_manager: crate::api::plugins::SharedPluginManager = {
            let mut pm = crate::plugins::PluginManager::new();
            let dirs = crate::plugins::default_plugin_dirs();
            let loaded = pm.load_all(&dirs);
            if !loaded.is_empty() {
                tracing::info!(count = loaded.len(), plugins = ?loaded, "loaded plugins");
            }
            Arc::new(tokio::sync::Mutex::new(pm))
        };

        // Collect plugin skill directories before taking the lock
        let plugin_skill_dirs = {
            let pm = plugin_manager.lock().await;
            pm.skill_dirs()
        };

        // Discover and sync skills from all roots (including plugin skills)
        let skill_repo = SkillRepo::new(self.db.clone());
        {
            let mut registry = crate::skills::SkillRegistry::new(
                self.config.skills.managed_dir.clone(),
            );
            match registry.discover_all_roots(&self.config.skills) {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!(count, "discovered skills");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "skill discovery failed");
                }
            }
            // Scan plugin skill directories
            if !plugin_skill_dirs.is_empty() {
                match registry.scan_plugin_dirs(&plugin_skill_dirs, &self.config.skills) {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!(count, "discovered plugin skills");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "plugin skill discovery failed");
                    }
                }
            }
            if let Err(e) = crate::skills::sync_discovered_skills(&skill_repo, &registry) {
                tracing::warn!(error = %e, "skill sync failed");
            }
        }

        // Load enabled skills for system prompt injection
        let enabled_skills = skill_repo.list_enabled().unwrap_or_default();
        if !enabled_skills.is_empty() {
            tracing::info!(count = enabled_skills.len(), "loaded enabled skills");
        }

        let system_prompt = crate::prompt::build_system_prompt(
            self.config.persona.name(),
            self.config.persona.system_prompt().unwrap_or_default(),
            &enabled_skills,
        );
        let model_id = self.config.llm_model.clone();

        // Initialize BYOK key resolver if Synapse API is configured
        let (key_resolver, jwt_cache) = if let (Some(api_url), Some(secret)) =
            (&self.config.synapse_api_url, &self.config.synapse_gateway_secret)
        {
            tracing::info!(url = %api_url, "BYOK enabled via Synapse");
            let resolver = Arc::new(crate::providers::KeyResolver::new(
                api_url.clone(),
                secret.clone(),
                self.config.api_keys.clone(),
            ));
            let auth_url = self.config.auth_base_url.clone()
                .unwrap_or_else(|| api_url.clone());
            let jwks = Arc::new(crate::api::jwt::JwksCache::new(auth_url));
            (Some(resolver), Some(jwks))
        } else {
            (None, None)
        };

        // Initialize key provisioner if Synapse API is configured
        let key_provisioner =
            if let (Some(api_url), Some(secret)) = (&self.config.synapse_api_url, &self.config.synapse_gateway_secret)
            {
                tracing::info!(url = %api_url, "key provisioning enabled via Synapse API");
                Some(Arc::new(crate::providers::KeyProvisioner::new(
                    api_url.clone(),
                    secret.clone(),
                )))
            } else {
                None
            };

        // Resolve knowledge packs from Manifold and merge with inline chunks
        let resolved_knowledge = if !self.config.persona.knowledge.packs.is_empty() {
            let manifold_url = self.config.api_server.manifold_url.as_deref()
                .unwrap_or("https://api.manifold.omni.dev");
            let resolver = crate::knowledge::KnowledgePackResolver::new(
                manifold_url,
                self.config.knowledge_cache_dir.clone(),
            );
            let results = resolver.resolve_all(&self.config.persona.knowledge.packs).await;
            let mut extra_chunks = Vec::new();
            for result in results {
                match result {
                    Ok(pack) => {
                        tracing::info!(name = %pack.name, chunks = pack.chunks.len(), "loaded knowledge pack");
                        extra_chunks.extend(pack.chunks);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to resolve knowledge pack");
                    }
                }
            }
            extra_chunks
        } else {
            Vec::new()
        };

        // Merge inline knowledge with resolved pack knowledge
        let mut all_knowledge = self.config.persona.knowledge.inline.clone();
        all_knowledge.extend(resolved_knowledge);

        if synapse.is_none() {
            tracing::info!("running in setup mode - chat unavailable until Synapse is reachable");
        }

        // Spawn periodic memory sync if configured
        if let Some(ref sync_config) = self.config.sync {
            let sync_client = crate::sync::SyncClient::new(
                &sync_config.api_url,
                &sync_config.device_id,
            );
            let sync_db = self.db.clone();
            let sync_interval = sync_config.interval_secs;

            tracing::info!(
                api_url = %sync_config.api_url,
                device_id = %sync_config.device_id,
                interval_secs = sync_interval,
                "memory sync enabled"
            );

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(sync_interval));
                // Skip the first immediate tick
                interval.tick().await;

                loop {
                    interval.tick().await;
                    if let Err(e) = sync_client.full_sync(&sync_db).await {
                        tracing::warn!(error = %e, "memory sync failed");
                    }
                }
            });
        }

        // Set up shutdown signal
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = shutdown_tx_clone.send(()).await;
            }
        });

        // Initialize Telegram channel if configured
        // Webhook mode when BEACON_PUBLIC_URL is set, polling mode otherwise
        let telegram_token = self.config.api_keys.telegram.clone();
        let telegram_public_url = self.config.api_server.public_url.clone();
        let mut telegram_polling_rx: Option<tokio::sync::mpsc::Receiver<IncomingMessage>> = None;

        let telegram = if let Some(token) = &telegram_token {
            if telegram_public_url.is_some() {
                // Webhook mode: create simple channel for API state
                let mut tg = TelegramChannel::new(token.clone());
                if let Err(e) = tg.connect().await {
                    tracing::error!(error = %e, "Telegram connect failed");
                    None
                } else {
                    // Auto-register webhook
                    let webhook_url = format!(
                        "{}/api/webhooks/telegram",
                        telegram_public_url.as_ref().unwrap().trim_end_matches('/')
                    );
                    if let Err(e) = tg.set_webhook(&webhook_url).await {
                        tracing::error!(error = %e, "Telegram webhook registration failed");
                    }
                    Some(tg)
                }
            } else {
                // Polling mode: create channel with receiver
                let (mut tg, rx) = TelegramChannel::with_receiver(token.clone());
                if let Err(e) = tg.connect().await {
                    tracing::error!(error = %e, "Telegram connect failed");
                    None
                } else {
                    tracing::info!("Telegram: no BEACON_PUBLIC_URL, using polling mode");
                    telegram_polling_rx = Some(rx);
                    Some(tg)
                }
            }
        } else {
            None
        };

        // Sync bot commands with Telegram (skill-derived command menu)
        if let Some(ref tg) = telegram {
            let skill_commands: Vec<crate::channels::BotCommand> = skill_repo
                .list_enabled_for_user(None)
                .unwrap_or_default()
                .iter()
                .filter(|s| s.skill.metadata.user_invocable)
                .filter_map(|s| {
                    s.command_name.as_ref().map(|cmd| crate::channels::BotCommand {
                        command: cmd.clone(),
                        description: s.skill.metadata.description.clone(),
                    })
                })
                .take(100) // Telegram limit
                .collect();

            if !skill_commands.is_empty()
                && let Err(e) = tg.sync_commands(&skill_commands).await
            {
                tracing::warn!(error = %e, "failed to sync Telegram bot commands");
            }
        }

        // Initialize vision client for image analysis (if Anthropic key available)
        let vision = self.config.api_keys.anthropic.as_ref().and_then(|key| {
            VisionClient::new(key.clone())
                .map(Arc::new)
                .ok()
        });

        // Create attachment processor with vision and Synapse (for audio transcription)
        let attachment_processor = Arc::new(AttachmentProcessor::new(
            vision,
            synapse.as_ref().map(Arc::clone),
            self.config.voice.stt_model.clone(),
        ));

        // Construct local key store for self-hosted provider management
        let local_key_store = crate::providers::LocalKeyStore::new(self.db.clone());

        // Start HTTP API server
        let persona_system_prompt = self.config.persona.system_prompt().map(String::from);
        let mut api_builder = ApiServerBuilder::new(
            self.db.clone(),
            self.config.persona.id().to_string(),
            self.config.persona.name().to_string(),
            persona_system_prompt,
            self.config.persona_cache_dir.clone(),
            self.config.api_server.port,
            Arc::clone(&tool_policy),
        )
        .api_key(self.config.api_server.api_key.clone())
        .manifold_url(self.config.api_server.manifold_url.clone())
        .static_dir(self.config.api_server.static_dir.clone())
        .persona_knowledge(all_knowledge.clone())
        .max_context_tokens(self.config.persona.memory.max_context_tokens)
        .knowledge_cache_dir(self.config.knowledge_cache_dir.clone())
        .plugin_manager(plugin_manager.clone())
        .cloud_mode(self.config.cloud_mode)
        .skills_config(self.config.skills.clone());

        if let Some(provisioner) = key_provisioner {
            api_builder = api_builder.key_provisioner(provisioner);
        }

        // Only set synapse client if configured
        if let Some(ref synapse) = synapse {
            api_builder = api_builder.synapse(Arc::clone(synapse));
        }
        api_builder = api_builder
            .llm_model(model_id.clone())
            .llm_max_tokens(MAX_TOKENS)
            .system_prompt(system_prompt.clone());

        // Clone for API state; keep original for polling handler
        let telegram_for_polling = telegram.clone();
        if let Some(tg) = telegram {
            api_builder = api_builder.telegram(tg);
        }
        if let Some(ref tg_config) = self.config.telegram {
            api_builder = api_builder.telegram_config(tg_config.clone());
        }
        if let Some(ref ct) = cron_tools {
            api_builder = api_builder.cron_tools(Arc::clone(ct));
        }

        // Initialize session compactor when Synapse is available
        if let Some(ref synapse) = synapse {
            let compact_config = crate::context::CompactionConfig::from_env();
            let compactor = Arc::new(crate::context::SessionCompactor::new(
                compact_config,
                Arc::clone(synapse),
                model_id.clone(),
            ));
            api_builder = api_builder.session_compactor(compactor);
        }

        api_builder = api_builder.voice_config(&self.config.voice);

        if let Some(model_info) = model_info {
            api_builder = api_builder.model_info(model_info);
        }

        if let Some(resolver) = key_resolver {
            api_builder = api_builder.key_resolver(resolver);
        }

        if let Some(jwks) = jwt_cache {
            api_builder = api_builder.jwt_cache(jwks);
        }

        api_builder = api_builder.local_key_store(local_key_store);

        // Initialize pairing manager (before API build so webhook can use it)
        let pairing_manager = Arc::new(PairingManager::new(self.config.dm_policy, self.db.clone()));
        tracing::info!(policy = %self.config.dm_policy, "DM security policy");

        // Initialize hook manager (before API build so webhook can use it)
        let hook_manager = Arc::new(HookManager::new(&self.config.hooks, &self.config.data_dir));

        api_builder = api_builder
            .hook_manager(Arc::clone(&hook_manager))
            .pairing_manager(Arc::clone(&pairing_manager))
            .attachment_processor(Arc::clone(&attachment_processor));

        let api_server = api_builder.build();
        let _api_handle = api_server.spawn();
        tracing::info!(port = self.config.api_server.port, "API server started");

        // Start channel handlers (only if synapse is configured)
        if let Some(ref synapse) = synapse {
            self.start_channels(
                Arc::clone(synapse),
                model_id.clone(),
                system_prompt.clone(),
                MAX_TOKENS,
                Arc::clone(&tool_policy),
                Arc::clone(&pairing_manager),
                Arc::clone(&attachment_processor),
                Arc::clone(&hook_manager),
                plugin_manager.clone(),
                all_knowledge,
                telegram_for_polling,
                telegram_polling_rx,
            )
            .await;
        } else {
            tracing::info!("skipping channel handlers - no synapse configured");
        }

        // Run voice loop on main thread (cpal streams aren't Send)
        // Only run if voice is enabled AND synapse is configured
        if self.config.voice.enabled && synapse.is_some() {
            self.run_voice_loop(
                Arc::clone(synapse.as_ref().unwrap()),
                model_id,
                system_prompt,
                MAX_TOKENS,
                Arc::clone(&tool_policy),
                &mut shutdown_rx,
                plugin_manager,
            )
            .await?;
        } else {
            if self.config.voice.enabled && synapse.is_none() {
                tracing::info!("voice disabled - no synapse configured");
            } else {
                tracing::info!("voice disabled - running in messaging-only mode");
            }
            shutdown_rx.recv().await;
        }

        tracing::info!("daemon stopped");
        Ok(())
    }

    /// Start channel message handlers
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn start_channels(
        &self,
        synapse: Arc<SynapseClient>,
        model_id: String,
        system_prompt: String,
        max_tokens: u32,
        tool_policy: Arc<crate::tools::ToolPolicy>,
        pairing_manager: Arc<PairingManager>,
        attachment_processor: Arc<AttachmentProcessor>,
        hook_manager: Arc<HookManager>,
        plugin_manager: crate::api::plugins::SharedPluginManager,
        knowledge_chunks: Vec<crate::persona::KnowledgeChunk>,
        telegram: Option<TelegramChannel>,
        telegram_polling_rx: Option<tokio::sync::mpsc::Receiver<IncomingMessage>>,
    ) {
        let persona_id = self.config.persona.id().to_string();
        let persona_system_prompt = self.config.persona.system_prompt().map(String::from);
        let max_context_tokens = self.config.persona.memory.max_context_tokens;

        // Discord
        if let Some(token) = &self.config.api_keys.discord {
            let (mut discord, rx) = DiscordChannel::with_receiver(token.clone());

            if let Err(e) = discord.connect().await {
                tracing::error!(error = %e, "Discord connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "discord",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        discord,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Slack
        if let Some(token) = &self.config.api_keys.slack {
            let (mut slack, rx) = SlackChannel::with_receiver(token.clone());

            if let Err(e) = slack.connect().await {
                tracing::error!(error = %e, "Slack connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "slack",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        slack,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // WhatsApp
        if let (Some(token), Some(phone_id)) = (
            &self.config.api_keys.whatsapp,
            &self.config.api_keys.whatsapp_phone_id,
        ) {
            let (mut whatsapp, rx) =
                WhatsAppChannel::with_receiver(token.clone(), phone_id.clone());

            if let Err(e) = whatsapp.connect().await {
                tracing::error!(error = %e, "WhatsApp connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "whatsapp",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        whatsapp,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Signal
        if let (Some(api_url), Some(phone)) = (
            &self.config.api_keys.signal_api_url,
            &self.config.api_keys.signal_phone,
        ) {
            let (mut signal, rx) = SignalChannel::with_receiver(api_url.clone(), phone.clone());

            if let Err(e) = signal.connect().await {
                tracing::error!(error = %e, "Signal connect failed");
            } else {
                // Spawn polling loop to fetch incoming messages
                signal.start_polling(std::time::Duration::from_secs(5));

                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "signal",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        signal,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // iMessage (macOS only)
        #[cfg(target_os = "macos")]
        if self.config.imessage.enabled {
            let (mut imessage, rx) = IMessageChannel::with_receiver(
                self.config.imessage.cli_path.clone(),
                self.config.imessage.db_path.clone(),
                self.config.imessage.region.clone(),
                self.config.imessage.service.clone(),
            );

            if let Err(e) = imessage.connect().await {
                tracing::error!(error = %e, "iMessage connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "imessage",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        imessage,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Matrix
        if let (Some(homeserver), Some(access_token), Some(user_id)) = (
            &self.config.api_keys.matrix_homeserver,
            &self.config.api_keys.matrix_access_token,
            &self.config.api_keys.matrix_user_id,
        ) {
            let (mut matrix, rx) = MatrixChannel::with_receiver(
                homeserver.clone(),
                access_token.clone(),
                user_id.clone(),
            );

            if let Err(e) = matrix.connect().await {
                tracing::error!(error = %e, "Matrix connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "matrix",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        matrix,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Microsoft Teams
        if let (Some(tenant_id), Some(client_id), Some(client_secret), Some(bot_id)) = (
            &self.config.api_keys.teams_tenant_id,
            &self.config.api_keys.teams_client_id,
            &self.config.api_keys.teams_client_secret,
            &self.config.api_keys.teams_bot_id,
        ) {
            let (mut teams, rx) = TeamsChannel::with_receiver(
                tenant_id.clone(),
                client_id.clone(),
                client_secret.clone(),
                bot_id.clone(),
            );

            if let Err(e) = teams.connect().await {
                tracing::error!(error = %e, "Teams connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "teams",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        teams,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Google Chat
        if let Some(service_account_path) = &self.config.api_keys.google_chat_service_account {
            let (mut google_chat, rx) = GoogleChatChannel::with_receiver(service_account_path.clone());

            if let Err(e) = google_chat.connect().await {
                tracing::error!(error = %e, "Google Chat connect failed");
            } else {
                let synapse = Arc::clone(&synapse);
                let model_id = model_id.clone();
                let system_prompt = system_prompt.clone();
                let session_repo = SessionRepo::new(self.db.clone());
                let user_repo = UserRepo::new(self.db.clone());
                let memory_repo = db::MemoryRepo::new(self.db.clone());
                let persona_id = persona_id.clone();
                let persona_system_prompt = persona_system_prompt.clone();
                let policy = Arc::clone(&tool_policy);
                let pairing = Arc::clone(&pairing_manager);
                let attachments = Arc::clone(&attachment_processor);
                let hooks = Arc::clone(&hook_manager);
                let knowledge = knowledge_chunks.clone();
                let pm = plugin_manager.clone();
                tokio::spawn(async move {
                    handle_channel_messages(
                        "google_chat",
                        rx,
                        synapse,
                        model_id,
                        system_prompt,
                        max_tokens,
                        google_chat,
                        session_repo,
                        user_repo,
                        memory_repo,
                        persona_id,
                        persona_system_prompt,
                        policy,
                        pairing,
                        attachments,
                        hooks,
                        knowledge,
                        max_context_tokens,
                        pm,
                        None,
                    )
                    .await;
                });
            }
        }

        // Telegram (polling mode — only when no public URL for webhooks)
        if let (Some(tg), Some(rx)) = (telegram, telegram_polling_rx) {
            tg.start_polling(std::time::Duration::from_secs(1));

            let synapse = Arc::clone(&synapse);
            let model_id = model_id.clone();
            let system_prompt = system_prompt.clone();
            let session_repo = SessionRepo::new(self.db.clone());
            let user_repo = UserRepo::new(self.db.clone());
            let memory_repo = db::MemoryRepo::new(self.db.clone());
            let persona_id = persona_id.clone();
            let persona_system_prompt = persona_system_prompt.clone();
            let policy = Arc::clone(&tool_policy);
            let pairing = Arc::clone(&pairing_manager);
            let attachments = Arc::clone(&attachment_processor);
            let hooks = Arc::clone(&hook_manager);
            let knowledge = knowledge_chunks.clone();
            let pm = plugin_manager.clone();
            let tg_config = self.config.telegram.clone();
            tokio::spawn(async move {
                handle_channel_messages(
                    "telegram",
                    rx,
                    synapse,
                    model_id,
                    system_prompt,
                    max_tokens,
                    tg,
                    session_repo,
                    user_repo,
                    memory_repo,
                    persona_id,
                    persona_system_prompt,
                    policy,
                    pairing,
                    attachments,
                    hooks,
                    knowledge,
                    max_context_tokens,
                    pm,
                    tg_config,
                )
                .await;
            });
        }
    }

    /// Run voice processing loop
    #[allow(clippy::future_not_send)]
    async fn run_voice_loop(
        &self,
        synapse: Arc<SynapseClient>,
        model_id: String,
        system_prompt: String,
        max_tokens: u32,
        tool_policy: Arc<crate::tools::ToolPolicy>,
        shutdown_rx: &mut mpsc::Receiver<()>,
        plugin_manager: crate::api::plugins::SharedPluginManager,
    ) -> Result<()> {
        // Available for future tool filtering by channel policy
        let _ = &tool_policy;

        let wake_word = self
            .config
            .persona
            .wake_word()
            .ok_or_else(|| Error::Config("voice.wakeWord required for voice mode".to_string()))?;
        let persona_id = self.config.persona.id();

        let stt_model = self.config.voice.stt_model.clone();
        let tts_model = self.config.voice.tts_model.clone();
        let tts_voice = self.config.voice.tts_voice.clone();
        let tts_speed = self.config.voice.tts_speed;

        let mut detector = WakeWordDetector::new(vec![wake_word.to_string()])?;
        let mut capture = AudioCapture::new()?;
        let mut playback = AudioPlayback::new()?;

        // Load life.json context for voice user
        let voice_context = self.config.life_json_path.as_ref().and_then(|path| {
            crate::context::LifeJsonReader::read(path).ok().map(|lj| {
                let ctx = lj.build_context_string(persona_id);
                tracing::debug!(path = %path.display(), "loaded life.json for voice");
                ctx
            })
        });

        capture.start()?;
        tracing::info!(wake_word, "listening for wake word");

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("shutdown requested");
                    break;
                }
                () = tokio::time::sleep(Duration::from_millis(100)) => {
                    if let Err(e) = self.process_voice_chunk(
                        &capture,
                        &mut playback,
                        &mut detector,
                        &synapse,
                        &model_id,
                        &system_prompt,
                        max_tokens,
                        &stt_model,
                        &tts_model,
                        &tts_voice,
                        tts_speed,
                        voice_context.as_deref(),
                        &plugin_manager,
                    ).await {
                        tracing::error!(error = %e, "voice processing error");
                    }
                }
            }
        }

        capture.stop();
        Ok(())
    }

    /// Process a chunk of voice audio
    #[allow(clippy::future_not_send, clippy::too_many_arguments)]
    async fn process_voice_chunk(
        &self,
        capture: &AudioCapture,
        playback: &mut AudioPlayback,
        detector: &mut WakeWordDetector,
        synapse: &Arc<SynapseClient>,
        model_id: &str,
        system_prompt: &str,
        max_tokens: u32,
        stt_model: &str,
        tts_model: &str,
        tts_voice: &str,
        tts_speed: f64,
        voice_context: Option<&str>,
        plugin_manager: &crate::api::plugins::SharedPluginManager,
    ) -> Result<()> {
        let samples = capture.take_buffer();

        if samples.len() < CHUNK_SIZE {
            return Ok(());
        }

        let speech_detected = detector.process(&samples);
        // Safe to unwrap: wake_word is validated in run_voice_loop
        let wake_word = self.config.persona.wake_word().unwrap_or("hey");

        if speech_detected && !detector.is_activated() {
            let speech_samples = detector.take_speech_buffer();
            capture.clear_buffer();

            if speech_samples.len() > SAMPLE_RATE as usize / 2 {
                tracing::debug!(samples = speech_samples.len(), "checking for wake word");

                let wav = samples_to_wav(&speech_samples, SAMPLE_RATE)?;
                let transcription = synapse
                    .transcribe(wav.into(), "audio.wav", stt_model)
                    .await;
                if let Ok(result) = transcription {
                    tracing::debug!(transcript = %result.text, "transcribed");

                    if detector.check_wake_word(&result.text) {
                        let command = extract_command(&result.text, wake_word);
                        if command.is_empty() {
                            speak(playback, synapse, tts_model, tts_voice, tts_speed, "Yes?").await?;
                        } else {
                            handle_voice_command(playback, synapse, model_id, system_prompt, max_tokens, tts_model, tts_voice, tts_speed, &command, voice_context, plugin_manager).await?;
                        }
                        detector.reset();
                    }
                }
            }
        } else if detector.is_activated() && detector.is_utterance_complete() {
            let speech_samples = detector.take_speech_buffer();
            capture.clear_buffer();

            let wav = samples_to_wav(&speech_samples, SAMPLE_RATE)?;
            match synapse.transcribe(wav.into(), "audio.wav", stt_model).await {
                Ok(result) => {
                    tracing::info!(command = %result.text, "command received");
                    handle_voice_command(playback, synapse, model_id, system_prompt, max_tokens, tts_model, tts_voice, tts_speed, &result.text, voice_context, plugin_manager).await?;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "STT failed");
                    speak(playback, synapse, tts_model, tts_voice, tts_speed, "Sorry, I didn't catch that").await?;
                }
            }
            detector.reset();
        } else if samples.len() > SAMPLE_RATE as usize * 5 {
            capture.clear_buffer();
        }

        Ok(())
    }
}

/// Result of pairing check
enum PairingResult {
    /// Sender is allowed to message
    Allowed,
    /// Sender is denied (allowlist mode, not on list)
    Denied,
    /// Pairing code sent, waiting for verification
    PendingPairing,
}

/// Check if sender is allowed based on DM policy
async fn check_pairing<C: Channel>(
    pairing_manager: &PairingManager,
    msg: &IncomingMessage,
    channel_name: &str,
    channel: &C,
) -> PairingResult {
    // Check if sender is allowed
    let allowed = match pairing_manager.is_allowed(&msg.sender_id, channel_name) {
        Ok(allowed) => allowed,
        Err(e) => {
            tracing::error!(error = %e, "pairing check failed, defaulting to deny");
            return PairingResult::Denied;
        }
    };

    if allowed {
        return PairingResult::Allowed;
    }

    // Handle based on policy
    match pairing_manager.policy() {
        DmPolicy::Open => PairingResult::Allowed,

        DmPolicy::Allowlist => {
            tracing::debug!(
                sender = %msg.sender_id,
                channel = channel_name,
                "sender not on allowlist, ignoring message"
            );
            PairingResult::Denied
        }

        DmPolicy::Pairing => {
            // Check if message is a pairing code verification attempt
            let trimmed = msg.content.trim().to_uppercase();
            if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
                // Try to verify the code
                match pairing_manager.verify_pairing(&msg.sender_id, channel_name, &trimmed) {
                    Ok(true) => {
                        // Send success message
                        let response = OutgoingMessage {
                            channel_id: msg.channel_id.clone(),
                            content: "Pairing successful! You can now send messages.".to_string(),
                            reply_to: None,
                            thread_id: None,
                            keyboard: None,
                            media: vec![],
                            edit_target: None,
                            voice_note: false,
                        };
                        if let Err(e) = channel.send(response).await {
                            tracing::warn!(error = %e, "failed to send pairing success message");
                        }
                        return PairingResult::Allowed;
                    }
                    Ok(false) => {
                        tracing::debug!(sender = %msg.sender_id, "invalid pairing code");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "pairing verification failed");
                    }
                }
            }

            // Generate pairing code for new sender
            match pairing_manager.generate_pairing_code(&msg.sender_id, channel_name) {
                Ok(Some(code)) => {
                    let response = OutgoingMessage {
                        channel_id: msg.channel_id.clone(),
                        content: format!(
                            "Please enter the pairing code to start messaging.\n\nYour code: {code}\n\n(This code expires in 10 minutes)"
                        ),
                        reply_to: None,
                        thread_id: None,
                        keyboard: None,
                        media: vec![],
                        edit_target: None,
                        voice_note: false,
                    };
                    if let Err(e) = channel.send(response).await {
                        tracing::warn!(error = %e, "failed to send pairing code");
                    }
                    tracing::info!(
                        sender = %msg.sender_id,
                        channel = channel_name,
                        "pairing code sent"
                    );
                }
                Ok(None) => {
                    // Already paired, shouldn't happen since is_allowed returned false
                    tracing::warn!(sender = %msg.sender_id, "unexpected state: paired but not allowed");
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to generate pairing code");
                }
            }

            PairingResult::PendingPairing
        }
    }
}

/// Accumulated tool call from streaming chunks
#[derive(Default)]
struct DaemonPendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Handle incoming messages from a channel
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_channel_messages<C: Channel + Send + 'static>(
    channel_name: &str,
    mut rx: mpsc::Receiver<IncomingMessage>,
    synapse: Arc<SynapseClient>,
    model_id: String,
    system_prompt: String,
    max_tokens: u32,
    channel: C,
    session_repo: SessionRepo,
    user_repo: UserRepo,
    memory_repo: crate::db::MemoryRepo,
    persona_id: String,
    persona_system_prompt: Option<String>,
    tool_policy: Arc<crate::tools::ToolPolicy>,
    pairing_manager: Arc<PairingManager>,
    attachment_processor: Arc<AttachmentProcessor>,
    hook_manager: Arc<HookManager>,
    knowledge_chunks: Vec<crate::persona::KnowledgeChunk>,
    max_context_tokens: usize,
    plugin_manager: crate::api::plugins::SharedPluginManager,
    telegram_config: Option<crate::config::TelegramConfig>,
) {
    // Available for future tool filtering by channel policy
    let _ = &tool_policy;

    tracing::info!(channel = channel_name, "channel handler started");

    while let Some(msg) = rx.recv().await {
        // Check DM security policy
        match check_pairing(&pairing_manager, &msg, channel_name, &channel).await {
            PairingResult::Allowed => (),
            PairingResult::Denied | PairingResult::PendingPairing => continue,
        }

        // Hook: message:received - can skip processing or provide auto-reply
        let hook_event = HookEvent::new(HookAction::MessageReceived, channel_name, &msg);
        let hook_result = hook_manager.trigger(&hook_event).await;

        if hook_result.skip_processing {
            tracing::debug!(channel = channel_name, "hook skipped processing");
            continue;
        }

        // Send hook auto-reply if provided (but continue processing unless skip_agent)
        if let Some(ref reply) = hook_result.reply {
            let outgoing = OutgoingMessage {
                channel_id: msg.channel_id.clone(),
                content: reply.clone(),
                reply_to: Some(msg.id.clone()),
                thread_id: None,
                keyboard: None,
                media: vec![],
                edit_target: None,
                voice_note: false,
            };
            if let Err(e) = channel.send(outgoing).await {
                tracing::error!(error = %e, "hook reply send error");
            }
            if hook_result.skip_agent {
                continue;
            }
        }

        // Find or create user and session
        let user = match user_repo.find_or_create(&msg.sender_id) {
            Ok(u) => u,
            Err(e) => {
                tracing::error!(error = %e, "failed to find/create user");
                continue;
            }
        };

        let session =
            match session_repo.find_or_create(&user.id, channel_name, &msg.channel_id, &persona_id)
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "failed to find/create session");
                    continue;
                }
            };

        // Publish beacon.conversation.started for new sessions (best-effort)
        match session_repo.message_count(&session.id) {
            Ok(0) => {
                crate::events::publish(crate::events::build_conversation_started_event(
                    &session.id,
                    channel_name,
                    &msg.sender_id,
                ));
            }
            Ok(_) => {} // existing session, don't re-publish started
            Err(e) => {
                tracing::warn!("failed to check message count for session {}: {}", session.id, e);
            }
        }

        // Extract thread_id for threading support
        // For platforms like Slack/Discord, reply_to contains the thread identifier
        let thread_id = msg.reply_to.as_deref();

        // Store user message with thread context
        if let Err(e) = session_repo.add_message_with_thread(
            &session.id,
            MessageRole::User,
            &msg.content,
            thread_id,
        ) {
            tracing::warn!(error = %e, "failed to store user message");
        }

        // Build context from life.json + session history + memories
        // Filter by thread if message is part of a thread
        let context_config = ContextConfig {
            max_messages: 20,
            max_tokens: 4000,
            persona_id: persona_id.clone(),
            max_memories: 10,
            persona_system_prompt: persona_system_prompt.clone(),
        };
        let context_builder = ContextBuilder::new(context_config);
        let mut built_context = context_builder.build_with_thread(
            &session.id,
            &user.id,
            user.life_json_path.as_deref(),
            &session_repo,
            &user_repo,
            Some((&memory_repo, msg.content.as_str())),
            thread_id,
        );

        if let Ok(ctx) = &built_context {
            tracing::debug!(
                session = %session.id,
                estimated_tokens = ctx.estimated_tokens,
                message_count = ctx.messages.len(),
                has_system_context = !ctx.system_context.is_empty(),
                "built conversation context"
            );
        }

        tracing::info!(
            channel = channel_name,
            session = %session.id,
            sender = %msg.sender_name,
            content = %msg.content,
            attachments = msg.attachments.len(),
            thread_id = ?thread_id,
            "message received"
        );

        // Publish beacon.message.received event (best-effort)
        crate::events::publish(
            crate::events::OmniEvent::new(
                "beacon.message.received",
                &msg.sender_id,
                serde_json::json!({
                    "channel": channel_name,
                    "messageId": msg.id,
                    "userId": msg.sender_id,
                }),
            )
            .with_subject(&msg.sender_id),
        );

        // Acknowledge message with reaction (configurable for Telegram)
        let reaction_level = telegram_config
            .as_ref()
            .filter(|_| channel_name == "telegram")
            .map_or(crate::config::ReactionLevel::Ack, |c| c.reaction_level);
        let ack_emoji = telegram_config
            .as_ref()
            .filter(|_| channel_name == "telegram")
            .map_or("\u{1F440}", |c| c.ack_reaction.as_str());

        if reaction_level != crate::config::ReactionLevel::Off {
            if let Err(e) = channel.add_reaction(&msg.channel_id, &msg.id, ack_emoji).await {
                tracing::debug!(error = %e, "ack reaction failed");
            }
        }

        // Process attachments to augment message content
        let content_with_attachments = if msg.attachments.is_empty() {
            msg.content.clone()
        } else {
            // Process attachments (images via vision, audio via STT)
            let attachment_text = attachment_processor
                .process_attachments(&msg.attachments)
                .await
                .unwrap_or_default();

            if attachment_text.is_empty() {
                msg.content.clone()
            } else {
                format!("{}\n\n{attachment_text}", msg.content)
            }
        };

        // Inject knowledge based on user message
        if let Ok(ref mut ctx) = built_context {
            if !knowledge_chunks.is_empty() {
                let max_knowledge_tokens = max_context_tokens / 4;
                let selected = crate::knowledge::select_knowledge(
                    &knowledge_chunks,
                    &content_with_attachments,
                    max_knowledge_tokens,
                );
                if !selected.is_empty() {
                    ctx.knowledge_context = crate::knowledge::format_knowledge(&selected);
                }
            }
        }

        // Build augmented prompt with context and history
        let augmented_prompt = match &built_context {
            Ok(ctx) => ctx.format_prompt(&content_with_attachments),
            Err(_) => content_with_attachments,
        };

        // Show typing indicator while processing
        if let Err(e) = channel.send_typing(&msg.channel_id).await {
            tracing::debug!(error = %e, "typing indicator failed");
        }

        // Hook: message:before_agent - can provide direct reply or skip agent
        let hook_event = HookEvent::new(HookAction::BeforeAgent, channel_name, &msg)
            .with_session(&session.id);
        let hook_result = hook_manager.trigger(&hook_event).await;

        // If hook provides a reply and wants to skip agent, send and move on
        if hook_result.skip_agent && hook_result.reply.is_some() {
            let reply = hook_result.reply.unwrap();
            let outgoing = OutgoingMessage {
                channel_id: msg.channel_id.clone(),
                content: reply,
                reply_to: Some(msg.id.clone()),
                thread_id: None,
                keyboard: None,
                media: vec![],
                edit_target: None,
                voice_note: false,
            };
            if let Err(e) = channel.send(outgoing).await {
                tracing::error!(error = %e, "hook reply send error");
            }
            continue;
        }

        // Send hook reply if provided (but continue to agent)
        if let Some(hook_reply) = hook_result.reply {
            let outgoing = OutgoingMessage {
                channel_id: msg.channel_id.clone(),
                content: hook_reply,
                reply_to: Some(msg.id.clone()),
                thread_id: None,
                keyboard: None,
                media: vec![],
                edit_target: None,
                voice_note: false,
            };
            if let Err(e) = channel.send(outgoing).await {
                tracing::error!(error = %e, "hook reply send error");
            }
        }

        // Fetch available tools from Synapse MCP and plugins
        let tools = {
            let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(&synapse), plugin_manager.clone());
            executor.list_tools().await.ok()
        };

        let use_streaming = channel.capabilities().contains(&ChannelCapability::Streaming);

        // Start streaming placeholder if channel supports it
        let streaming_msg_id = if use_streaming {
            channel
                .send_streaming_start(
                    &msg.channel_id,
                    "\u{2026}",
                    Some(&msg.id),
                    msg.thread_id.as_deref(),
                )
                .await
                .ok()
        } else {
            None
        };

        // Process with Synapse (multi-turn tool loop)
        let response = {
            let mut llm_messages = vec![
                synapse_client::Message::system(&system_prompt),
                synapse_client::Message::user(&augmented_prompt),
            ];
            let mut final_response = String::new();
            let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(&synapse), plugin_manager.clone());
            let mut loop_detector = crate::tools::loop_detection::LoopDetector::default();

            for _turn in 0..10 {
                let request = synapse_client::ChatRequest {
                    model: model_id.clone(),
                    messages: llm_messages.clone(),
                    stream: use_streaming,
                    temperature: None,
                    top_p: None,
                    max_tokens: Some(max_tokens),
                    stop: None,
                    tools: tools.clone(),
                    tool_choice: None,
                };

                if use_streaming {
                    // Streaming path: use chat_completion_stream
                    match synapse.chat_completion_stream(&request).await {
                        Ok(mut stream) => {
                            let mut turn_text = String::new();
                            let mut pending_tool_calls: Vec<DaemonPendingToolCall> = Vec::new();
                            let mut finish_reason: Option<String> = None;

                            while let Some(event) = stream.next().await {
                                match event {
                                    Ok(synapse_client::ChatEvent::ContentDelta(text)) => {
                                        turn_text.push_str(&text);
                                        if let Some(ref mid) = streaming_msg_id {
                                            let _ = channel
                                                .send_streaming_update(&msg.channel_id, mid, &turn_text)
                                                .await;
                                        }
                                    }
                                    Ok(synapse_client::ChatEvent::ToolCallStart { index, id, name }) => {
                                        let idx = index as usize;
                                        while pending_tool_calls.len() <= idx {
                                            pending_tool_calls.push(DaemonPendingToolCall::default());
                                        }
                                        pending_tool_calls[idx].id = id;
                                        pending_tool_calls[idx].name = name;
                                    }
                                    Ok(synapse_client::ChatEvent::ToolCallDelta { index, arguments }) => {
                                        let idx = index as usize;
                                        if idx < pending_tool_calls.len() {
                                            pending_tool_calls[idx].arguments.push_str(&arguments);
                                        }
                                    }
                                    Ok(synapse_client::ChatEvent::Done { finish_reason: fr, .. }) => {
                                        finish_reason = fr;
                                        break;
                                    }
                                    Ok(synapse_client::ChatEvent::Error(e)) => {
                                        tracing::error!(error = %e, "streaming error");
                                        break;
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, "stream event error");
                                        break;
                                    }
                                }
                            }

                            if !turn_text.is_empty() {
                                final_response.push_str(&turn_text);
                            }

                            // Handle tool calls from streaming
                            if finish_reason.as_deref() == Some("tool_calls") && !pending_tool_calls.is_empty() {
                                let tool_calls: Vec<synapse_client::ToolCall> = pending_tool_calls
                                    .iter()
                                    .map(|tc| synapse_client::ToolCall {
                                        id: tc.id.clone(),
                                        tool_type: "function".to_owned(),
                                        function: synapse_client::FunctionCall {
                                            name: tc.name.clone(),
                                            arguments: tc.arguments.clone(),
                                        },
                                    })
                                    .collect();

                                let assistant_content = if turn_text.is_empty() {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::Value::String(turn_text)
                                };

                                llm_messages.push(synapse_client::Message {
                                    role: "assistant".to_owned(),
                                    content: assistant_content,
                                    tool_calls: Some(tool_calls.clone()),
                                    tool_call_id: None,
                                });

                                let mut should_break = false;
                                for tc in &tool_calls {
                                    let result = executor
                                        .execute(&tc.function.name, &tc.function.arguments)
                                        .await
                                        .unwrap_or_else(|e| format!("Error: {e}"));

                                    let severity = loop_detector.record(
                                        &tc.function.name,
                                        &tc.function.arguments,
                                        &result,
                                    );
                                    match severity {
                                        crate::tools::loop_detection::LoopSeverity::CircuitBreaker => {
                                            tracing::warn!(tool = %tc.function.name, "circuit breaker: tool loop detected");
                                            llm_messages.push(synapse_client::Message::tool(
                                                &tc.id,
                                                "Error: Circuit breaker triggered — this tool has been called too many times with the same arguments. Please try a different approach.",
                                            ));
                                            should_break = true;
                                            break;
                                        }
                                        crate::tools::loop_detection::LoopSeverity::Critical => {
                                            tracing::warn!(tool = %tc.function.name, "critical: possible tool loop");
                                            llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                            llm_messages.push(synapse_client::Message::system(
                                                "Warning: You appear to be in a loop calling the same tool repeatedly. Please try a different approach or provide a final answer.",
                                            ));
                                        }
                                        crate::tools::loop_detection::LoopSeverity::Warning => {
                                            tracing::info!(tool = %tc.function.name, "warning: repeated tool call pattern");
                                            llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                        }
                                        crate::tools::loop_detection::LoopSeverity::None => {
                                            llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                        }
                                    }

                                    let tool_success = !result.starts_with("Error: ");
                                    crate::events::publish(
                                        crate::events::build_tool_executed_event(
                                            &session.id,
                                            &tc.function.name,
                                            tool_success,
                                            &msg.sender_id,
                                        ),
                                    );
                                }

                                if should_break {
                                    break;
                                }
                                continue;
                            }

                            break;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "synapse stream error");
                            final_response =
                                "Sorry, I encountered an error processing your message.".to_string();
                            break;
                        }
                    }
                } else {
                    // Non-streaming path: use chat_completion
                    match synapse.chat_completion(&request).await {
                        Ok(resp) => {
                            let choice = match resp.choices.first() {
                                Some(c) => c,
                                None => break,
                            };

                            if let Some(ref text) = choice.message.content {
                                final_response.push_str(text);
                            }

                            if choice.finish_reason.as_deref() == Some("tool_calls") {
                                if let Some(ref tool_calls) = choice.message.tool_calls {
                                    let assistant_content = choice
                                        .message
                                        .content
                                        .as_ref()
                                        .map(|t| serde_json::Value::String(t.clone()))
                                        .unwrap_or(serde_json::Value::Null);

                                    llm_messages.push(synapse_client::Message {
                                        role: "assistant".to_owned(),
                                        content: assistant_content,
                                        tool_calls: Some(tool_calls.clone()),
                                        tool_call_id: None,
                                    });

                                    let mut should_break = false;
                                    for tc in tool_calls {
                                        let result = executor
                                            .execute(&tc.function.name, &tc.function.arguments)
                                            .await
                                            .unwrap_or_else(|e| format!("Error: {e}"));

                                        let severity = loop_detector.record(
                                            &tc.function.name,
                                            &tc.function.arguments,
                                            &result,
                                        );
                                        match severity {
                                            crate::tools::loop_detection::LoopSeverity::CircuitBreaker => {
                                                tracing::warn!(
                                                    tool = %tc.function.name,
                                                    "circuit breaker: tool loop detected, breaking"
                                                );
                                                llm_messages.push(synapse_client::Message::tool(
                                                    &tc.id,
                                                    "Error: Circuit breaker triggered — this tool has been called too many times with the same arguments. Please try a different approach.",
                                                ));
                                                should_break = true;
                                                break;
                                            }
                                            crate::tools::loop_detection::LoopSeverity::Critical => {
                                                tracing::warn!(
                                                    tool = %tc.function.name,
                                                    "critical: possible tool loop detected"
                                                );
                                                llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                                llm_messages.push(synapse_client::Message::system(
                                                    "Warning: You appear to be in a loop calling the same tool repeatedly. Please try a different approach or provide a final answer.",
                                                ));
                                            }
                                            crate::tools::loop_detection::LoopSeverity::Warning => {
                                                tracing::info!(
                                                    tool = %tc.function.name,
                                                    "warning: repeated tool call pattern detected"
                                                );
                                                llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                            }
                                            crate::tools::loop_detection::LoopSeverity::None => {
                                                llm_messages.push(synapse_client::Message::tool(&tc.id, &result));
                                            }
                                        }

                                        let tool_success = !result.starts_with("Error: ");
                                        crate::events::publish(
                                            crate::events::build_tool_executed_event(
                                                &session.id,
                                                &tc.function.name,
                                                tool_success,
                                                &msg.sender_id,
                                            ),
                                        );
                                    }

                                    if should_break {
                                        break;
                                    }
                                    continue;
                                }
                            }

                            break;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "synapse error");
                            final_response =
                                "Sorry, I encountered an error processing your message."
                                    .to_string();
                            break;
                        }
                    }
                }
            }

            // Finalize streaming message
            if let Some(ref mid) = streaming_msg_id {
                let _ = channel
                    .send_streaming_end(&msg.channel_id, mid, &final_response)
                    .await;
            }

            final_response
        };

        // Hook: message:after_agent - can modify response
        let hook_event = HookEvent::new(HookAction::AfterAgent, channel_name, &msg)
            .with_session(&session.id)
            .with_response(&response);
        let hook_result = hook_manager.trigger(&hook_event).await;
        let response = hook_result.modified_response.unwrap_or(response);

        // Store assistant response with thread context
        if let Err(e) = session_repo.add_message_with_thread(
            &session.id,
            MessageRole::Assistant,
            &response,
            thread_id,
        ) {
            tracing::warn!(error = %e, "failed to store assistant message");
        }

        // Send response (skip if streaming already delivered it)
        if streaming_msg_id.is_none() {
            let outgoing = OutgoingMessage {
                channel_id: msg.channel_id.clone(),
                content: response,
                reply_to: thread_id.map(String::from).or_else(|| Some(msg.id.clone())),
                thread_id: None,
                keyboard: None,
                media: vec![],
                edit_target: None,
                voice_note: false,
            };

            if let Err(e) = channel.send(outgoing).await {
                tracing::error!(error = %e, "send error");
            }
        }

        // Mark complete with reaction (configurable for Telegram)
        if reaction_level != crate::config::ReactionLevel::Off {
            let done_emoji = telegram_config
                .as_ref()
                .filter(|_| channel_name == "telegram")
                .map_or("\u{2705}", |c| c.done_reaction.as_str());
            if let Err(e) = channel.add_reaction(&msg.channel_id, &msg.id, done_emoji).await {
                tracing::debug!(error = %e, "done reaction failed");
            }
        }

        // Publish beacon.message.processed event (best-effort)
        crate::events::publish(
            crate::events::OmniEvent::new(
                "beacon.message.processed",
                &msg.sender_id,
                serde_json::json!({
                    "channel": channel_name,
                    "messageId": msg.id,
                    "userId": msg.sender_id,
                }),
            )
            .with_subject(&msg.sender_id),
        );

        // Publish beacon.conversation.ended event (best-effort)
        // For daemon channels, one request-response exchange = one conversation
        crate::events::publish(crate::events::build_conversation_ended_event(
            &session.id,
            channel_name,
            &msg.sender_id,
        ));
    }
}

/// Handle a voice command
#[allow(clippy::too_many_arguments)]
async fn handle_voice_command(
    playback: &mut AudioPlayback,
    synapse: &Arc<SynapseClient>,
    model_id: &str,
    system_prompt: &str,
    max_tokens: u32,
    tts_model: &str,
    tts_voice: &str,
    tts_speed: f64,
    command: &str,
    voice_context: Option<&str>,
    plugin_manager: &crate::api::plugins::SharedPluginManager,
) -> Result<()> {
    tracing::info!(command, "processing voice command");

    // TODO: inject knowledge into voice path
    let prompt = match voice_context {
        Some(ctx) if !ctx.is_empty() => {
            format!("<user-context>\n{ctx}\n</user-context>\n\n{command}")
        }
        _ => command.to_string(),
    };

    // Fetch available tools from Synapse MCP and plugins
    let tools = {
        let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(synapse), plugin_manager.clone());
        executor.list_tools().await.ok()
    };

    let mut messages = vec![
        synapse_client::Message::system(system_prompt),
        synapse_client::Message::user(&prompt),
    ];
    let mut final_text = String::new();
    let executor = crate::tools::executor::ToolExecutor::new(Arc::clone(synapse), plugin_manager.clone());

    for _turn in 0..10 {
        let request = synapse_client::ChatRequest {
            model: model_id.to_string(),
            messages: messages.clone(),
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: Some(max_tokens),
            stop: None,
            tools: tools.clone(),
            tool_choice: None,
        };

        let response = synapse
            .chat_completion(&request)
            .await
            .map_err(|e| Error::Agent(e.to_string()))?;

        let choice = match response.choices.first() {
            Some(c) => c,
            None => break,
        };

        // Overwrite each turn so we speak only the final answer
        if let Some(ref text) = choice.message.content {
            final_text = text.clone();
        }

        if choice.finish_reason.as_deref() == Some("tool_calls") {
            if let Some(ref tool_calls) = choice.message.tool_calls {
                let assistant_content = choice
                    .message
                    .content
                    .as_ref()
                    .map(|t| serde_json::Value::String(t.clone()))
                    .unwrap_or(serde_json::Value::Null);

                messages.push(synapse_client::Message {
                    role: "assistant".to_owned(),
                    content: assistant_content,
                    tool_calls: Some(tool_calls.clone()),
                    tool_call_id: None,
                });

                for tc in tool_calls {
                    let result = executor
                        .execute(&tc.function.name, &tc.function.arguments)
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}"));

                    messages.push(synapse_client::Message::tool(&tc.id, &result));
                }

                continue;
            }
        }

        break;
    }

    tracing::debug!(response_len = final_text.len(), "synapse responded");
    speak(playback, synapse, tts_model, tts_voice, tts_speed, &final_text).await
}

/// Speak via Synapse TTS
async fn speak(
    playback: &mut AudioPlayback,
    synapse: &SynapseClient,
    tts_model: &str,
    tts_voice: &str,
    tts_speed: f64,
    text: &str,
) -> Result<()> {
    tracing::debug!(text, "speaking");
    let request = synapse_client::SpeechRequest {
        model: tts_model.to_string(),
        input: text.to_string(),
        voice: tts_voice.to_string(),
        response_format: None,
        speed: Some(tts_speed),
    };
    let audio = synapse
        .synthesize(&request)
        .await
        .map_err(|e| Error::Tts(e.to_string()))?;
    playback.play_mp3(&audio).await
}

/// Extract command after wake word
fn extract_command(transcript: &str, wake_word: &str) -> String {
    let lower = transcript.to_lowercase();
    let wake_lower = wake_word.to_lowercase();

    lower.find(&wake_lower).map_or_else(
        || transcript.to_string(),
        |pos| {
            transcript[pos + wake_word.len()..]
                .trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == '.')
                .to_string()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_command() {
        assert_eq!(
            extract_command("Hey Orin, what's the weather?", "hey orin"),
            "what's the weather?"
        );
        assert_eq!(extract_command("Hey Orin", "hey orin"), "");
    }
}
