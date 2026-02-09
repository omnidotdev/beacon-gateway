//! Daemon - the main gateway service
//!
//! Orchestrates voice capture, wake word detection, STT, agent, TTS, and messaging channels

use std::sync::Arc;
use std::time::Duration;

use omni_cli::Agent;
use omni_cli::core::agent::providers::{AnthropicProvider, OpenAiProvider};
use tokio::sync::{Mutex, mpsc};

use crate::api::{ApiServerBuilder, ModelInfo};
use crate::attachments::{AttachmentProcessor, VisionClient};
use crate::channels::{
    Channel, DiscordChannel, GoogleChatChannel, IncomingMessage, MatrixChannel,
    OutgoingMessage, SignalChannel, SlackChannel, TeamsChannel, TelegramChannel, WhatsAppChannel,
};
#[cfg(target_os = "macos")]
use crate::channels::IMessageChannel;
use crate::context::{ContextBuilder, ContextConfig};
use crate::db::{self, DbPool, MessageRole, SessionRepo, UserRepo};
use crate::security::{DmPolicy, PairingManager};
use crate::voice::{
    AudioCapture, AudioPlayback, SAMPLE_RATE, SpeechToText, TextToSpeech, WakeWordDetector,
    samples_to_wav,
};
use crate::hooks::{HookAction, HookEvent, HookManager};
use crate::{Config, Error, Result};

/// Audio processing chunk size (100ms at 16kHz)
const CHUNK_SIZE: usize = 1600;

/// Default Anthropic model
pub(crate) const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-20250514";

/// Default `OpenAI` model (fallback)
pub(crate) const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";

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

    /// Initialize the LLM agent if API keys are available
    ///
    /// Returns (agent, model_info) - both None if no API keys are configured.
    /// The gateway can still run in "unconfigured" mode for provider setup.
    fn init_agent(&self) -> (Option<Arc<Mutex<Agent>>>, Option<ModelInfo>) {
        let preferred_provider = self.config.llm_provider.as_deref();

        let provider_result: Option<(Box<dyn omni_cli::core::agent::LlmProvider>, &str)> =
            match preferred_provider {
                // Force OpenAI
                Some("openai") => {
                    if let Some(openai_key) = self.config.api_keys.openai.as_ref() {
                        match OpenAiProvider::new(openai_key.clone()) {
                            Ok(p) => {
                                tracing::info!(model = DEFAULT_OPENAI_MODEL, provider = "openai", "using OpenAI (configured)");
                                Some((Box::new(p), DEFAULT_OPENAI_MODEL))
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "failed to initialize OpenAI provider");
                                None
                            }
                        }
                    } else {
                        tracing::warn!("BEACON_LLM_PROVIDER=openai but OPENAI_API_KEY not set");
                        None
                    }
                }
                // Force Anthropic
                Some("anthropic") => {
                    if let Some(anthropic_key) = self.config.api_keys.anthropic.as_ref() {
                        match AnthropicProvider::new(anthropic_key.clone()) {
                            Ok(p) => {
                                tracing::info!(model = DEFAULT_ANTHROPIC_MODEL, provider = "anthropic", "using Anthropic (configured)");
                                Some((Box::new(p), DEFAULT_ANTHROPIC_MODEL))
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "failed to initialize Anthropic provider");
                                None
                            }
                        }
                    } else {
                        tracing::warn!("BEACON_LLM_PROVIDER=anthropic but ANTHROPIC_API_KEY not set");
                        None
                    }
                }
                // Auto-detect: prefer Anthropic, fall back to OpenAI
                _ => {
                    if let Some(anthropic_key) = &self.config.api_keys.anthropic {
                        match AnthropicProvider::new(anthropic_key.clone()) {
                            Ok(p) => {
                                tracing::info!(model = DEFAULT_ANTHROPIC_MODEL, provider = "anthropic", "using Anthropic");
                                Some((Box::new(p), DEFAULT_ANTHROPIC_MODEL))
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Anthropic provider failed, trying OpenAI");
                                // Try OpenAI as fallback
                                if let Some(openai_key) = &self.config.api_keys.openai {
                                    match OpenAiProvider::new(openai_key.clone()) {
                                        Ok(p) => {
                                            tracing::info!(model = DEFAULT_OPENAI_MODEL, provider = "openai", "using OpenAI (fallback)");
                                            Some((Box::new(p), DEFAULT_OPENAI_MODEL))
                                        }
                                        Err(e) => {
                                            tracing::error!(error = %e, "failed to initialize OpenAI provider");
                                            None
                                        }
                                    }
                                } else {
                                    None
                                }
                            }
                        }
                    } else if let Some(openai_key) = &self.config.api_keys.openai {
                        match OpenAiProvider::new(openai_key.clone()) {
                            Ok(p) => {
                                tracing::info!(model = DEFAULT_OPENAI_MODEL, provider = "openai", "using OpenAI (no Anthropic key)");
                                Some((Box::new(p), DEFAULT_OPENAI_MODEL))
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "failed to initialize OpenAI provider");
                                None
                            }
                        }
                    } else {
                        tracing::warn!("no LLM provider configured - gateway running in setup mode");
                        tracing::info!("configure a provider via the API or set ANTHROPIC_API_KEY / OPENAI_API_KEY");
                        None
                    }
                }
            };

        match provider_result {
            Some((provider, model_id)) => {
                let system_prompt = build_system_prompt(&self.config);
                let agent = Arc::new(Mutex::new(Agent::with_system(
                    provider,
                    model_id,
                    MAX_TOKENS,
                    system_prompt,
                )));

                let model_info = ModelInfo {
                    model_id: model_id.to_string(),
                    provider: if model_id.starts_with("claude") { "anthropic" } else { "openai" }.to_string(),
                };

                tracing::debug!(model = model_id, "agent initialized");
                (Some(agent), Some(model_info))
            }
            None => (None, None),
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

        // Initialize agent with preferred LLM provider (optional - gateway can start unconfigured)
        // BEACON_LLM_PROVIDER can be "anthropic" or "openai" to force a specific provider
        let (agent, model_info) = self.init_agent();

        // Initialize BYOK key resolver if identity service is configured
        let (key_resolver, jwt_cache) = if let (Some(auth_url), Some(svc_key)) =
            (&self.config.auth_base_url, &self.config.service_key)
        {
            tracing::info!(url = %auth_url, "BYOK enabled via identity service");
            let resolver = Arc::new(crate::providers::KeyResolver::new(
                auth_url.clone(),
                svc_key.clone(),
                self.config.api_keys.clone(),
            ));
            let jwks = Arc::new(crate::api::jwt::JwksCache::new(auth_url.clone()));
            (Some(resolver), Some(jwks))
        } else {
            (None, None)
        };

        // Get tool policy from persona
        let tool_policy = Arc::new(self.config.persona.tool_policy());

        if !self.config.persona.knowledge.packs.is_empty() {
            tracing::warn!(
                count = self.config.persona.knowledge.packs.len(),
                "knowledge pack references not yet supported, will be ignored"
            );
        }

        if agent.is_none() {
            tracing::info!("running in setup mode - chat unavailable until provider configured");
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
        let telegram = if let Some(token) = &self.config.api_keys.telegram {
            let mut tg = TelegramChannel::new(token.clone());
            if let Err(e) = tg.connect().await {
                tracing::error!(error = %e, "Telegram connect failed");
                None
            } else {
                Some(tg)
            }
        } else {
            None
        };

        // Initialize voice services for API (if OpenAI key available)
        let (stt, tts) = if let Some(ref openai_key) = self.config.api_keys.openai {
            let stt = SpeechToText::new_whisper(openai_key.clone(), "whisper-1".to_string())
                .map(Arc::new)
                .ok();
            let tts_voice = self.config.persona.tts_voice().unwrap_or("alloy");
            let tts = TextToSpeech::new_openai(
                openai_key.clone(),
                tts_voice.to_string(),
                self.config.persona.tts_speed(),
            )
            .map(Arc::new)
            .ok();
            (stt, tts)
        } else {
            (None, None)
        };

        // Initialize vision client for image analysis (if Anthropic key available)
        let vision = self.config.api_keys.anthropic.as_ref().and_then(|key| {
            VisionClient::new(key.clone())
                .map(Arc::new)
                .ok()
        });

        // Create attachment processor with vision and STT
        let attachment_processor = Arc::new(AttachmentProcessor::new(vision, stt.clone()));

        // Start HTTP API server
        let persona_system_prompt = self.config.persona.system_prompt().map(String::from);
        let mut api_builder = ApiServerBuilder::new(
            self.db.clone(),
            self.config.persona.id().to_string(),
            persona_system_prompt,
            self.config.persona_cache_dir.clone(),
            self.config.api_server.port,
            Arc::clone(&tool_policy),
        )
        .api_key(self.config.api_server.api_key.clone())
        .manifold_url(self.config.api_server.manifold_url.clone())
        .static_dir(self.config.api_server.static_dir.clone())
        .persona_knowledge(self.config.persona.knowledge.inline.clone())
        .max_context_tokens(self.config.persona.memory.max_context_tokens);

        // Only set agent if configured
        if let Some(ref agent) = agent {
            api_builder = api_builder.agent(Arc::clone(agent));
        }

        if let Some(tg) = telegram {
            api_builder = api_builder.telegram(tg);
        }

        if let Some(stt) = stt {
            api_builder = api_builder.stt(stt);
        }

        if let Some(tts) = tts {
            api_builder = api_builder.tts(tts);
        }

        if let Some(model_info) = model_info {
            api_builder = api_builder.model_info(model_info);
        }

        if let Some(resolver) = key_resolver {
            api_builder = api_builder.key_resolver(resolver);
        }

        if let Some(jwks) = jwt_cache {
            api_builder = api_builder.jwt_cache(jwks);
        }

        let api_server = api_builder.build();
        let _api_handle = api_server.spawn();
        tracing::info!(port = self.config.api_server.port, "API server started");

        // Initialize pairing manager
        let pairing_manager = Arc::new(PairingManager::new(self.config.dm_policy, self.db.clone()));
        tracing::info!(policy = %self.config.dm_policy, "DM security policy");

        // Initialize hook manager
        let hook_manager = Arc::new(HookManager::new(&self.config.hooks, &self.config.data_dir));

        // Start channel handlers (only if agent is configured)
        if let Some(ref agent) = agent {
            self.start_channels(
                Arc::clone(agent),
                Arc::clone(&tool_policy),
                Arc::clone(&pairing_manager),
                Arc::clone(&attachment_processor),
                Arc::clone(&hook_manager),
            )
            .await;
        } else {
            tracing::info!("skipping channel handlers - no agent configured");
        }

        // Run voice loop on main thread (cpal streams aren't Send)
        // Only run if voice is enabled AND agent is configured
        if self.config.voice.enabled && agent.is_some() {
            self.run_voice_loop(Arc::clone(agent.as_ref().unwrap()), Arc::clone(&tool_policy), &mut shutdown_rx)
                .await?;
        } else {
            if self.config.voice.enabled && agent.is_none() {
                tracing::info!("voice disabled - no agent configured");
            } else {
                tracing::info!("voice disabled - running in messaging-only mode");
            }
            shutdown_rx.recv().await;
        }

        tracing::info!("daemon stopped");
        Ok(())
    }

    /// Start channel message handlers
    #[allow(clippy::too_many_lines)]
    async fn start_channels(
        &self,
        agent: Arc<Mutex<Agent>>,
        tool_policy: Arc<crate::tools::ToolPolicy>,
        pairing_manager: Arc<PairingManager>,
        attachment_processor: Arc<AttachmentProcessor>,
        hook_manager: Arc<HookManager>,
    ) {
        let persona_id = self.config.persona.id().to_string();
        let persona_system_prompt = self.config.persona.system_prompt().map(String::from);
        let knowledge_chunks = self.config.persona.knowledge.inline.clone();
        let max_context_tokens = self.config.persona.memory.max_context_tokens;

        // Discord
        if let Some(token) = &self.config.api_keys.discord {
            let (mut discord, rx) = DiscordChannel::with_receiver(token.clone());

            if let Err(e) = discord.connect().await {
                tracing::error!(error = %e, "Discord connect failed");
            } else {
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "discord",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "slack",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "whatsapp",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "signal",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "imessage",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "matrix",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "teams",
                        rx,
                        agent,
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
                let agent = Arc::clone(&agent);
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
                tokio::spawn(async move {
                    handle_channel_messages(
                        "google_chat",
                        rx,
                        agent,
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
                    )
                    .await;
                });
            }
        }
    }

    /// Run voice processing loop
    #[allow(clippy::future_not_send)]
    async fn run_voice_loop(
        &self,
        agent: Arc<Mutex<Agent>>,
        tool_policy: Arc<crate::tools::ToolPolicy>,
        shutdown_rx: &mut mpsc::Receiver<()>,
    ) -> Result<()> {
        let openai_key = self.config.api_keys.openai.as_ref().unwrap();
        let wake_word = self
            .config
            .persona
            .wake_word()
            .ok_or_else(|| Error::Config("voice.wakeWord required for voice mode".to_string()))?;
        let tts_voice = self
            .config
            .persona
            .tts_voice()
            .ok_or_else(|| Error::Config("voice.tts.voice required for voice mode".to_string()))?;
        let persona_id = self.config.persona.id();

        // Initialize STT based on config
        let stt = match self.config.voice.stt_provider.as_str() {
            "deepgram" => {
                let deepgram_key = self.config.api_keys.deepgram.as_ref().ok_or_else(|| {
                    Error::Config("DEEPGRAM_API_KEY required for Deepgram STT".to_string())
                })?;
                SpeechToText::new_deepgram(deepgram_key.clone(), "nova-2".to_string())?
            }
            _ => SpeechToText::new_whisper(openai_key.clone(), "whisper-1".to_string())?,
        };

        // Initialize TTS based on config
        let tts = match self.config.voice.tts_provider.as_str() {
            "elevenlabs" => {
                let elevenlabs_key = self.config.api_keys.elevenlabs.as_ref().ok_or_else(|| {
                    Error::Config("ELEVENLABS_API_KEY required for ElevenLabs TTS".to_string())
                })?;
                TextToSpeech::new_elevenlabs(elevenlabs_key.clone(), tts_voice.to_string())?
            }
            _ => TextToSpeech::new_openai(
                openai_key.clone(),
                tts_voice.to_string(),
                self.config.persona.tts_speed(),
            )?,
        };
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
                        &stt,
                        &tts,
                        &agent,
                        voice_context.as_deref(),
                        &tool_policy,
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
        stt: &SpeechToText,
        tts: &TextToSpeech,
        agent: &Arc<Mutex<Agent>>,
        voice_context: Option<&str>,
        tool_policy: &Arc<crate::tools::ToolPolicy>,
    ) -> Result<()> {
        let samples = capture.peek_buffer();

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
                if let Ok(transcript) = stt.transcribe(&wav).await {
                    tracing::debug!(transcript = %transcript, "transcribed");

                    if detector.check_wake_word(&transcript) {
                        let command = extract_command(&transcript, wake_word);
                        if command.is_empty() {
                            speak(playback, tts, "Yes?").await?;
                        } else {
                            handle_voice_command(playback, tts, agent, &command, voice_context, tool_policy).await?;
                        }
                        detector.reset();
                    }
                }
            }
        } else if detector.is_activated() && detector.is_utterance_complete() {
            let speech_samples = detector.take_speech_buffer();
            capture.clear_buffer();

            let wav = samples_to_wav(&speech_samples, SAMPLE_RATE)?;
            match stt.transcribe(&wav).await {
                Ok(transcript) => {
                    tracing::info!(command = %transcript, "command received");
                    handle_voice_command(playback, tts, agent, &transcript, voice_context, tool_policy).await?;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "STT failed");
                    speak(playback, tts, "Sorry, I didn't catch that").await?;
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

/// Handle incoming messages from a channel
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_channel_messages<C: Channel + Send + 'static>(
    channel_name: &str,
    mut rx: mpsc::Receiver<IncomingMessage>,
    agent: Arc<Mutex<Agent>>,
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
) {
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
            Some(&memory_repo),
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

        // Acknowledge message with eyes reaction
        if let Err(e) = channel.add_reaction(&msg.channel_id, &msg.id, "\u{1F440}").await {
            tracing::debug!(error = %e, "ack reaction failed");
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

        // If hook provides a reply and wants to skip agent, use that directly
        let response = if hook_result.skip_agent && hook_result.reply.is_some() {
            hook_result.reply.unwrap()
        } else {
            // Send hook reply if provided (but continue to agent)
            if let Some(hook_reply) = hook_result.reply {
                let outgoing = OutgoingMessage {
                    channel_id: msg.channel_id.clone(),
                    content: hook_reply,
                    reply_to: Some(msg.id.clone()),
                };
                if let Err(e) = channel.send(outgoing).await {
                    tracing::error!(error = %e, "hook reply send error");
                }
            }

            // Process with agent
            let mut agent = agent.lock().await;
            agent.clear();

            // Apply tool filter based on channel policy
            let allowed_tools = tool_policy.allowed_tools(channel_name);
            agent.set_tool_filter(Some(allowed_tools));

            match agent.chat(&augmented_prompt, |_| {}).await {
                Ok(response) => response,
                Err(e) => {
                    tracing::error!(error = %e, "agent error");
                    "Sorry, I encountered an error processing your message.".to_string()
                }
            }
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

        // Send response in the same thread (reply to original message)
        // For Slack: thread_ts causes reply to appear in thread
        // For Discord: reply_to creates a reply reference
        // For Matrix/Teams: similar thread continuation
        let outgoing = OutgoingMessage {
            channel_id: msg.channel_id.clone(),
            content: response,
            reply_to: thread_id.map(String::from).or_else(|| Some(msg.id.clone())),
        };

        if let Err(e) = channel.send(outgoing).await {
            tracing::error!(error = %e, "send error");
        }

        // Mark complete with checkmark reaction
        if let Err(e) = channel.add_reaction(&msg.channel_id, &msg.id, "\u{2705}").await {
            tracing::debug!(error = %e, "done reaction failed");
        }
    }
}

/// Handle a voice command
async fn handle_voice_command(
    playback: &mut AudioPlayback,
    tts: &TextToSpeech,
    agent: &Arc<Mutex<Agent>>,
    command: &str,
    voice_context: Option<&str>,
    tool_policy: &Arc<crate::tools::ToolPolicy>,
) -> Result<()> {
    tracing::info!(command, "processing voice command");

    // TODO: inject knowledge into voice path
    // Build prompt with context if available
    let prompt = match voice_context {
        Some(ctx) if !ctx.is_empty() => {
            format!("<user-context>\n{ctx}\n</user-context>\n\n{command}")
        }
        _ => command.to_string(),
    };

    let response = {
        let mut agent = agent.lock().await;
        agent.clear(); // Clear since we're not tracking voice sessions yet

        // Apply tool filter based on voice channel policy (full access)
        let allowed_tools = tool_policy.allowed_tools("voice");
        agent.set_tool_filter(Some(allowed_tools));

        agent
            .chat(&prompt, |_| {})
            .await
            .map_err(|e| Error::Agent(e.to_string()))?
    };

    tracing::debug!(response_len = response.len(), "agent responded");
    speak(playback, tts, &response).await
}

/// Speak via TTS
async fn speak(playback: &mut AudioPlayback, tts: &TextToSpeech, text: &str) -> Result<()> {
    tracing::debug!(text, "speaking");
    let audio = tts.synthesize(text).await?;
    playback.play_mp3(&audio).await
}

/// Build system prompt
fn build_system_prompt(config: &Config) -> String {
    let persona_prompt = config.persona.system_prompt().unwrap_or_default();
    let name = config.persona.name();

    if persona_prompt.is_empty() {
        format!("You are {name}. Keep responses concise and conversational.")
    } else {
        format!(
            "{persona_prompt}

Your name is {name}. Keep responses concise and conversational."
        )
    }
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
