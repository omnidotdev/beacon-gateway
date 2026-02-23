//! Configuration management for Beacon gateway

pub mod file;
#[cfg(feature = "embedded-synapse")]
pub mod synapse_bridge;

use std::path::PathBuf;

use crate::hooks::HooksConfig;
use crate::relay::RelayConfig;
use crate::security::{AuthConfig, DmPolicy};
use crate::{Error, Persona, Result};

/// Beacon gateway configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Active persona
    pub persona: Persona,

    /// Path to persona cache directory
    pub persona_cache_dir: PathBuf,

    /// Path to data directory (database, cache, etc)
    pub data_dir: PathBuf,

    /// Path to extensions directory
    pub extension_dir: PathBuf,

    /// Voice configuration
    pub voice: VoiceConfig,

    /// API keys
    pub api_keys: ApiKeys,

    /// Default life.json path for local/voice user
    pub life_json_path: Option<PathBuf>,

    /// HTTP API server configuration
    pub api_server: ApiServerConfig,

    /// Preferred LLM provider ("anthropic" or "openai")
    /// Set via `BEACON_LLM_PROVIDER` env var
    pub llm_provider: Option<String>,

    /// iMessage channel configuration (macOS only)
    pub imessage: IMessageConfig,

    /// DM security policy
    pub dm_policy: DmPolicy,

    /// Cloud relay configuration
    pub relay: RelayConfig,

    /// Gateway authentication configuration
    pub auth: AuthConfig,

    /// Hooks configuration
    pub hooks: HooksConfig,

    /// Identity service URL for JWT validation (JWKS endpoint).
    /// Used for `JwksCache` to validate Bearer tokens.
    /// BYOK key resolution now uses `synapse_api_url` instead.
    pub auth_base_url: Option<String>,

    /// Synapse API base URL for internal endpoints (key provisioning)
    pub synapse_api_url: Option<String>,

    /// Shared secret for authenticating to Synapse API internal endpoints
    pub synapse_gateway_secret: Option<String>,

    /// Synapse AI router URL
    pub synapse_url: String,

    /// LLM model identifier for chat completions
    pub llm_model: String,

    /// Cloud mode: requires JWT auth, enables rate limiting
    pub cloud_mode: bool,

    /// Directory for caching resolved knowledge packs
    pub knowledge_cache_dir: PathBuf,

    /// Memory sync configuration (optional, disabled by default)
    pub sync: Option<SyncConfig>,

    /// Skills system configuration
    pub skills: SkillsConfig,
}

/// HTTP API server configuration
#[derive(Debug, Clone)]
pub struct ApiServerConfig {
    /// Port to listen on
    pub port: u16,

    /// API key for admin endpoints (from `BEACON_API_KEY` env)
    pub api_key: Option<String>,

    /// Manifold registry URL for skills marketplace
    pub manifold_url: Option<String>,

    /// Vortex workflow automation URL
    pub vortex_url: Option<String>,

    /// Path to static files directory (web UI)
    pub static_dir: Option<PathBuf>,
}

/// Voice processing configuration
#[derive(Debug, Clone, Default)]
pub struct VoiceConfig {
    /// Enable voice input
    pub enabled: bool,

    /// STT model for Synapse (e.g. "whisper-1", "deepgram/nova-2")
    pub stt_model: String,

    /// TTS model for Synapse (e.g. "tts-1", "elevenlabs/eleven_monolingual_v1")
    pub tts_model: String,

    /// TTS voice identifier
    pub tts_voice: String,

    /// TTS speed multiplier (0.25 to 4.0)
    pub tts_speed: f64,
}

/// iMessage channel configuration (macOS only)
#[derive(Debug, Clone, Default)]
pub struct IMessageConfig {
    /// Enable iMessage channel
    pub enabled: bool,

    /// Path to imsg CLI (defaults to "imsg")
    pub cli_path: Option<String>,

    /// Optional database path override
    pub db_path: Option<String>,

    /// Region for phone number normalization (defaults to "US")
    pub region: Option<String>,

    /// Service preference: "iMessage", "SMS", or "auto"
    pub service: Option<String>,
}

/// API keys for external services
#[derive(Debug, Clone, Default)]
pub struct ApiKeys {
    /// `OpenAI` API key (for Whisper and TTS)
    pub openai: Option<String>,

    /// `Anthropic` API key (for agent)
    pub anthropic: Option<String>,

    /// `OpenRouter` API key (unified access to multiple LLM providers)
    /// See: <https://openrouter.ai/keys>
    pub openrouter: Option<String>,

    /// `ElevenLabs` API key (optional TTS)
    pub elevenlabs: Option<String>,

    /// `Deepgram` API key (optional STT)
    pub deepgram: Option<String>,

    /// Discord bot token
    pub discord: Option<String>,

    /// Slack bot token
    pub slack: Option<String>,

    /// Telegram bot token
    pub telegram: Option<String>,

    /// `WhatsApp` Business API access token
    pub whatsapp: Option<String>,

    /// `WhatsApp` phone number ID
    pub whatsapp_phone_id: Option<String>,

    /// Signal CLI REST API URL (e.g., `<http://localhost:8080>`)
    pub signal_api_url: Option<String>,

    /// Signal phone number (e.g., "+1234567890")
    pub signal_phone: Option<String>,

    /// Matrix homeserver URL (e.g., `<https://matrix.org>`)
    pub matrix_homeserver: Option<String>,

    /// Matrix access token
    pub matrix_access_token: Option<String>,

    /// Matrix user ID (e.g., "@bot:matrix.org")
    pub matrix_user_id: Option<String>,

    /// Microsoft Teams tenant ID (Azure AD)
    pub teams_tenant_id: Option<String>,

    /// Microsoft Teams client ID (Bot application ID)
    pub teams_client_id: Option<String>,

    /// Microsoft Teams client secret
    pub teams_client_secret: Option<String>,

    /// Microsoft Teams bot ID
    pub teams_bot_id: Option<String>,

    /// Google Chat service account JSON file path
    pub google_chat_service_account: Option<std::path::PathBuf>,
}

/// Skills system configuration
#[derive(Debug, Clone)]
pub struct SkillsConfig {
    /// Directory for managed (filesystem) skills
    pub managed_dir: PathBuf,
    /// Max skills included in the system prompt
    pub max_skills_in_prompt: usize,
    /// Max total chars from skills in the system prompt
    pub max_skills_prompt_chars: usize,
    /// Max bytes per individual skill file
    pub max_skill_file_bytes: usize,
    /// Additional skill directories to scan (lowest precedence after bundled)
    pub extra_dirs: Vec<PathBuf>,
    /// Personal agent skills directory (~/.agents/skills/)
    pub personal_dir: PathBuf,
    /// Bundled skill allowlist. Empty = all allowed
    pub allow_bundled: Vec<String>,
    /// Install automation preferences
    pub install_prefs: crate::skills::SkillInstallPreferences,
    /// Max candidate directories to scan per root
    pub max_candidates_per_root: usize,
    /// Max skills to load per source directory
    pub max_skills_per_source: usize,
    /// Agent-level skill filter (include/exclude patterns)
    pub skill_filter: crate::skills::SkillFilter,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            managed_dir: default_skills_dir(),
            max_skills_in_prompt: 50,
            max_skills_prompt_chars: 30_000,
            max_skill_file_bytes: 256_000,
            extra_dirs: Vec::new(),
            personal_dir: default_personal_dir(),
            allow_bundled: Vec::new(),
            install_prefs: crate::skills::SkillInstallPreferences::default(),
            max_candidates_per_root: 1000,
            max_skills_per_source: 200,
            skill_filter: crate::skills::SkillFilter::default(),
        }
    }
}

/// Default managed skills directory: `~/.config/omni/beacon/skills/`
fn default_skills_dir() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".config/omni/beacon/skills"),
        |d| d.config_dir().join("omni").join("beacon").join("skills"),
    )
}

/// Default personal agent skills directory: `~/.agents/skills/`
fn default_personal_dir() -> PathBuf {
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".agents/skills"),
        |d| d.home_dir().join(".agents").join("skills"),
    )
}

/// Memory sync configuration
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Cloud API URL for memory sync
    pub api_url: String,

    /// Unique device identifier for this gateway instance
    pub device_id: String,

    /// Sync interval in seconds (default: 300 = 5 minutes)
    pub interval_secs: u64,
}

/// Return the XDG cache directory for persona files, creating it if needed
///
/// Uses `~/.cache/omni/beacon/personas/` on Linux
pub fn persona_cache_dir() -> PathBuf {
    let cache_dir = directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".cache/omni/beacon/personas"),
        |d| d.cache_dir().join("omni").join("beacon").join("personas"),
    );

    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        tracing::warn!(
            path = %cache_dir.display(),
            error = %e,
            "failed to create persona cache directory"
        );
    }

    cache_dir
}

impl Config {
    /// Load configuration for a persona
    ///
    /// # Errors
    ///
    /// Returns error if persona file cannot be loaded
    pub fn load(persona_id: &str) -> Result<Self> {
        Self::load_with_options(persona_id, false)
    }

    /// Load configuration with explicit voice disable option
    ///
    /// # Errors
    ///
    /// Returns error if persona file cannot be loaded
    pub fn load_with_options(persona_id: &str, disable_voice: bool) -> Result<Self> {
        // Load optional TOML config file (env > toml > default)
        let fc = file::load_config_file();

        // Load persona with priority: env override → Manifold → cache → embedded
        let persona = Self::load_persona_with_priority(persona_id)?;
        let cache_dir = persona_cache_dir();

        // Load API keys (env > toml > None)
        let api_keys = ApiKeys {
            openai: std::env::var("OPENAI_API_KEY").ok().or(fc.api_keys.openai),
            anthropic: std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .or(fc.api_keys.anthropic),
            openrouter: std::env::var("OPENROUTER_API_KEY")
                .ok()
                .or(fc.api_keys.openrouter),
            elevenlabs: std::env::var("ELEVENLABS_API_KEY")
                .ok()
                .or(fc.api_keys.elevenlabs),
            deepgram: std::env::var("DEEPGRAM_API_KEY")
                .ok()
                .or(fc.api_keys.deepgram),
            discord: std::env::var("DISCORD_TOKEN").ok().or(fc.api_keys.discord),
            slack: std::env::var("SLACK_BOT_TOKEN").ok().or(fc.api_keys.slack),
            telegram: std::env::var("TELEGRAM_BOT_TOKEN")
                .ok()
                .or(fc.api_keys.telegram),
            whatsapp: std::env::var("WHATSAPP_TOKEN").ok(),
            whatsapp_phone_id: std::env::var("WHATSAPP_PHONE_ID").ok(),
            signal_api_url: std::env::var("SIGNAL_API_URL").ok(),
            signal_phone: std::env::var("SIGNAL_PHONE").ok(),
            matrix_homeserver: std::env::var("MATRIX_HOMESERVER").ok(),
            matrix_access_token: std::env::var("MATRIX_ACCESS_TOKEN").ok(),
            matrix_user_id: std::env::var("MATRIX_USER_ID").ok(),
            teams_tenant_id: std::env::var("TEAMS_TENANT_ID").ok(),
            teams_client_id: std::env::var("TEAMS_CLIENT_ID").ok(),
            teams_client_secret: std::env::var("TEAMS_CLIENT_SECRET").ok(),
            teams_bot_id: std::env::var("TEAMS_BOT_ID").ok(),
            google_chat_service_account: std::env::var("GOOGLE_CHAT_SERVICE_ACCOUNT")
                .ok()
                .map(std::path::PathBuf::from),
        };

        // API server config (env > toml > default)
        let api_server = ApiServerConfig {
            port: std::env::var("BEACON_API_PORT")
                .or_else(|_| std::env::var("PORT"))
                .ok()
                .and_then(|s| s.parse().ok())
                .or(fc.server.port)
                .unwrap_or(18790),
            api_key: std::env::var("BEACON_API_KEY").ok(),
            manifold_url: std::env::var("MANIFOLD_URL").ok(),
            vortex_url: std::env::var("VORTEX_URL").ok(),
            static_dir: std::env::var("BEACON_STATIC_DIR").ok().map(PathBuf::from),
        };

        // Voice config (env > toml > persona > default)
        let tts_voice = persona.tts_voice().unwrap_or("alloy").to_string();
        let tts_speed = f64::from(persona.tts_speed());
        let voice_enabled = if disable_voice {
            false
        } else {
            fc.voice.enabled.unwrap_or(true)
        };
        let voice = VoiceConfig {
            enabled: voice_enabled,
            stt_model: std::env::var("BEACON_STT_MODEL")
                .ok()
                .or(fc.voice.stt_model)
                .unwrap_or_else(|| "whisper-1".to_string()),
            tts_model: std::env::var("BEACON_TTS_MODEL")
                .ok()
                .or(fc.voice.tts_model)
                .unwrap_or_else(|| "tts-1".to_string()),
            tts_voice: fc.voice.tts_voice.unwrap_or(tts_voice),
            tts_speed: fc.voice.tts_speed.unwrap_or(tts_speed),
        };

        if disable_voice {
            tracing::info!("voice explicitly disabled via --disable-voice");
        }

        // Determine data directory (~/.local/share/omni/beacon on Linux)
        let data_dir = directories::BaseDirs::new()
            .map_or_else(|| PathBuf::from("."), |d| d.data_dir().join("omni").join("beacon"));

        // Ensure data dir exists
        std::fs::create_dir_all(&data_dir).ok();

        // Extension directory (~/.local/share/omni/beacon/extensions)
        let extension_dir = std::env::var("BEACON_EXTENSION_DIR").map_or_else(
            |_| {
                directories::BaseDirs::new().map_or_else(
                    || PathBuf::from(".local/share/omni/beacon/extensions"),
                    |dirs| {
                        dirs.data_dir()
                            .join("omni")
                            .join("beacon")
                            .join("extensions")
                    },
                )
            },
            PathBuf::from,
        );

        // Find life.json for local user
        let life_json_path = Self::find_life_json();

        // LLM provider preference (env > toml > None)
        let llm_provider = std::env::var("BEACON_LLM_PROVIDER")
            .ok()
            .or(fc.llm.provider);

        // iMessage config (env > toml > default)
        let imessage_toml = fc.channels.imessage.unwrap_or_default();
        let imessage = IMessageConfig {
            enabled: std::env::var("IMESSAGE_ENABLED")
                .ok()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .or(imessage_toml.enabled)
                .unwrap_or(false),
            cli_path: std::env::var("IMESSAGE_CLI_PATH")
                .ok()
                .or(imessage_toml.cli_path),
            db_path: std::env::var("IMESSAGE_DB_PATH")
                .ok()
                .or(imessage_toml.db_path),
            region: std::env::var("IMESSAGE_REGION")
                .ok()
                .or(imessage_toml.region),
            service: std::env::var("IMESSAGE_SERVICE")
                .ok()
                .or(imessage_toml.service),
        };

        // DM security policy (open, pairing, allowlist)
        let dm_policy = std::env::var("BEACON_DM_POLICY")
            .map(|s| DmPolicy::from_str(&s))
            .unwrap_or_default();

        // Cloud relay configuration
        let relay = RelayConfig::from_env();

        // Gateway authentication configuration
        let auth = AuthConfig::from_env();

        // Hooks configuration (from hooks.toml in data dir or defaults)
        let hooks = Self::load_hooks_config(&data_dir);

        // Identity service URL for JWT validation (JWKS endpoint)
        let auth_base_url = std::env::var("AUTH_BASE_URL").ok();

        // Synapse API (internal endpoints for key provisioning)
        let synapse_api_url = std::env::var("SYNAPSE_API_URL").ok();
        let synapse_gateway_secret = std::env::var("SYNAPSE_GATEWAY_SECRET").ok();

        // Synapse AI router (env > toml > default)
        let synapse_url = std::env::var("SYNAPSE_URL")
            .ok()
            .or(fc.server.synapse_url)
            .unwrap_or_else(|| "http://localhost:6000".to_string());
        let llm_model = std::env::var("BEACON_LLM_MODEL")
            .ok()
            .or(fc.llm.model)
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

        let cloud_mode = std::env::var("BEACON_CLOUD_MODE")
            .ok()
            .map(|v| v == "true" || v == "1")
            .or(fc.server.cloud_mode)
            .unwrap_or(false);

        // Knowledge pack cache directory
        let knowledge_cache_dir = std::env::var("BEACON_KNOWLEDGE_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                directories::BaseDirs::new().map_or_else(
                    || PathBuf::from(".cache/omni/beacon/knowledge"),
                    |d| d.cache_dir().join("omni").join("beacon").join("knowledge"),
                )
            });

        // Memory sync configuration (opt-in via env vars)
        let sync = std::env::var("BEACON_SYNC_API_URL").ok().map(|api_url| {
            let device_id = std::env::var("BEACON_DEVICE_ID")
                .unwrap_or_else(|_| format!("gw_{}", uuid::Uuid::new_v4()));
            let interval_secs = std::env::var("BEACON_SYNC_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);
            SyncConfig {
                api_url,
                device_id,
                interval_secs,
            }
        });

        // Skills configuration (env > toml > default)
        let skills = {
            let default = SkillsConfig::default();
            let toml_skills = &fc.skills;

            let extra_dirs = std::env::var("BEACON_SKILLS_EXTRA_DIRS")
                .ok()
                .map(|s| s.split(',').map(|p| PathBuf::from(p.trim())).collect())
                .or_else(|| {
                    toml_skills.extra_dirs.as_ref().map(|dirs| {
                        dirs.iter().map(|p| PathBuf::from(p)).collect()
                    })
                })
                .unwrap_or(default.extra_dirs);

            let personal_dir = std::env::var("BEACON_SKILLS_PERSONAL_DIR")
                .map(PathBuf::from)
                .or_else(|_| {
                    toml_skills.personal_dir.as_ref().map(PathBuf::from).ok_or(())
                })
                .unwrap_or(default.personal_dir);

            let allow_bundled = std::env::var("BEACON_ALLOW_BUNDLED")
                .ok()
                .map(|s| {
                    s.split(',')
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .or_else(|| toml_skills.allow_bundled.clone())
                .unwrap_or(default.allow_bundled);

            let prefer_brew = std::env::var("BEACON_SKILLS_PREFER_BREW")
                .ok()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .or(toml_skills.prefer_brew)
                .unwrap_or(default.install_prefs.prefer_brew);

            let node_manager = std::env::var("BEACON_SKILLS_NODE_MANAGER")
                .ok()
                .or_else(|| toml_skills.node_manager.clone())
                .and_then(|s| match s.to_lowercase().as_str() {
                    "npm" => Some(crate::skills::NodeManager::Npm),
                    "pnpm" => Some(crate::skills::NodeManager::Pnpm),
                    "yarn" => Some(crate::skills::NodeManager::Yarn),
                    "bun" => Some(crate::skills::NodeManager::Bun),
                    _ => None,
                })
                .unwrap_or(default.install_prefs.node_manager);

            let max_candidates_per_root = std::env::var("BEACON_MAX_CANDIDATES_PER_ROOT")
                .ok()
                .and_then(|s| s.parse().ok())
                .or(toml_skills.max_candidates_per_root)
                .unwrap_or(default.max_candidates_per_root);

            let max_skills_per_source = std::env::var("BEACON_MAX_SKILLS_PER_SOURCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .or(toml_skills.max_skills_per_source)
                .unwrap_or(default.max_skills_per_source);

            let skill_include = std::env::var("BEACON_SKILLS_INCLUDE")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
                .or_else(|| toml_skills.skill_include.clone())
                .unwrap_or_default();

            let skill_exclude = std::env::var("BEACON_SKILLS_EXCLUDE")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
                .or_else(|| toml_skills.skill_exclude.clone())
                .unwrap_or_default();

            SkillsConfig {
                managed_dir: std::env::var("BEACON_SKILLS_DIR")
                    .map(PathBuf::from)
                    .or_else(|_| toml_skills.managed_dir.as_ref().map(PathBuf::from).ok_or(()))
                    .unwrap_or(default.managed_dir),
                max_skills_in_prompt: std::env::var("BEACON_MAX_SKILLS_PROMPT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .or(toml_skills.max_skills_in_prompt)
                    .unwrap_or(default.max_skills_in_prompt),
                max_skills_prompt_chars: std::env::var("BEACON_MAX_SKILLS_CHARS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .or(toml_skills.max_skills_prompt_chars)
                    .unwrap_or(default.max_skills_prompt_chars),
                max_skill_file_bytes: toml_skills
                    .max_skill_file_bytes
                    .unwrap_or(default.max_skill_file_bytes),
                extra_dirs,
                personal_dir,
                allow_bundled,
                install_prefs: crate::skills::SkillInstallPreferences {
                    prefer_brew,
                    node_manager,
                },
                max_candidates_per_root,
                max_skills_per_source,
                skill_filter: crate::skills::SkillFilter {
                    include: skill_include,
                    exclude: skill_exclude,
                },
            }
        };

        Ok(Self {
            persona,
            persona_cache_dir: cache_dir,
            data_dir,
            extension_dir,
            voice,
            api_keys,
            life_json_path,
            api_server,
            llm_provider,
            imessage,
            dm_policy,
            relay,
            auth,
            hooks,
            auth_base_url,
            synapse_api_url,
            synapse_gateway_secret,
            synapse_url,
            llm_model,
            cloud_mode,
            knowledge_cache_dir,
            sync,
            skills,
        })
    }

    /// Find life.json in standard locations
    fn find_life_json() -> Option<PathBuf> {
        // 1. Environment variable
        if let Ok(path) = std::env::var("LIFE_JSON_PATH") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }

        // 2. Home directory ~/.life.json
        if let Some(home) = directories::UserDirs::new() {
            let home_path = home.home_dir().join(".life.json");
            if home_path.exists() {
                return Some(home_path);
            }
        }

        // 3. XDG config ~/.config/life.json
        if let Some(dirs) = directories::BaseDirs::new() {
            let xdg_path = dirs.config_dir().join("life.json");
            if xdg_path.exists() {
                return Some(xdg_path);
            }
        }

        None
    }

    /// Load hooks configuration from file or defaults
    fn load_hooks_config(data_dir: &std::path::Path) -> HooksConfig {
        // Check for hooks.toml in ~/.beacon or data_dir
        let config_paths = [
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".beacon/hooks.toml")),
            Some(data_dir.join("hooks.toml")),
        ];

        for path in config_paths.into_iter().flatten() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            tracing::info!(path = %path.display(), "loaded hooks config");
                            return config;
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to parse hooks config, using defaults"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to read hooks config"
                        );
                    }
                }
            }
        }

        // Check env var for enabled state
        let enabled = std::env::var("BEACON_HOOKS_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        HooksConfig {
            enabled,
            path: None,
            auto_reply: vec![],
        }
    }

    /// Load a persona with priority: env override, Manifold, cache, embedded
    ///
    /// # Errors
    ///
    /// Returns error if persona cannot be loaded from any source
    fn load_persona_with_priority(persona_id: &str) -> Result<Persona> {
        // 1. BEACON_PERSONAS_DIR env var (dev override)
        if let Ok(dir) = std::env::var("BEACON_PERSONAS_DIR") {
            let path = PathBuf::from(&dir);
            if path.exists() {
                match Self::load_persona(&path, persona_id) {
                    Ok(persona) => {
                        tracing::info!(
                            persona_id,
                            path = %path.display(),
                            "loaded persona from BEACON_PERSONAS_DIR"
                        );
                        return Ok(persona);
                    }
                    Err(e) => {
                        tracing::warn!(
                            persona_id,
                            error = %e,
                            "BEACON_PERSONAS_DIR set but persona not found, continuing"
                        );
                    }
                }
            } else {
                tracing::warn!(
                    path = %dir,
                    "BEACON_PERSONAS_DIR set but directory does not exist"
                );
            }
        }

        // 2. Manifold fetch (cache on success)
        let manifold_url = std::env::var("MANIFOLD_URL")
            .unwrap_or_else(|_| "https://api.manifold.omni.dev".to_string());
        let manifold_namespace = std::env::var("MANIFOLD_NAMESPACE")
            .unwrap_or_else(|_| "community".to_string());

        match Self::fetch_persona_from_manifold(&manifold_url, &manifold_namespace, persona_id) {
            Ok(persona) => {
                Self::cache_persona(persona_id, &persona);
                return Ok(persona);
            }
            Err(e) => {
                tracing::warn!(
                    persona_id,
                    error = %e,
                    "Manifold fetch failed, trying cache"
                );
            }
        }

        // 3. Local cache
        match Self::load_cached_persona(persona_id) {
            Ok(persona) => {
                tracing::info!(persona_id, "loaded persona from cache");
                return Ok(persona);
            }
            Err(e) => {
                tracing::debug!(
                    persona_id,
                    error = %e,
                    "no cached persona, trying embedded"
                );
            }
        }

        // 4. Embedded fallback
        Self::load_embedded_persona(persona_id)
    }

    /// Load a persona from file (JSON preferred, TOML fallback)
    fn load_persona(personas_dir: &std::path::Path, persona_id: &str) -> Result<Persona> {
        // Try JSON first (persona.json spec)
        let json_path = personas_dir.join(format!("{persona_id}.json"));
        if json_path.exists() {
            let content = std::fs::read_to_string(&json_path)?;
            let persona: Persona = serde_json::from_str(&content)
                .map_err(|e| Error::Config(format!("failed to parse {persona_id}.json: {e}")))?;
            tracing::debug!(path = %json_path.display(), "loaded persona from JSON");
            return Ok(persona);
        }

        // Fall back to TOML (legacy format)
        let toml_path = personas_dir.join(format!("{persona_id}.toml"));
        if toml_path.exists() {
            let content = std::fs::read_to_string(&toml_path)?;
            let persona: Persona = toml::from_str(&content)
                .map_err(|e| Error::Config(format!("failed to parse {persona_id}.toml: {e}")))?;
            tracing::warn!(
                path = %toml_path.display(),
                "loaded persona from legacy TOML format, consider migrating to JSON"
            );
            return Ok(persona);
        }

        Err(Error::PersonaNotFound(persona_id.to_string()))
    }

    /// Embedded default persona data for when no local files or Manifold are available
    const EMBEDDED_PERSONAS: &[(&str, &str)] = &[
        ("orin", include_str!("../../personas/orin.json")),
        ("microcap", include_str!("../../personas/microcap.json")),
    ];

    /// Load an embedded persona compiled into the binary
    ///
    /// # Errors
    ///
    /// Returns error if persona ID is not found in embedded data
    pub fn load_embedded_persona(persona_id: &str) -> Result<Persona> {
        Self::EMBEDDED_PERSONAS
            .iter()
            .find(|(id, _)| *id == persona_id)
            .and_then(|(_, json)| {
                let persona: Persona = serde_json::from_str(json).ok()?;
                tracing::info!(persona_id, "loaded persona from embedded data");
                Some(persona)
            })
            .ok_or_else(|| Error::PersonaNotFound(persona_id.to_string()))
    }

    /// Return the embedded persona array for enumeration
    #[must_use]
    pub const fn embedded_personas() -> &'static [(&'static str, &'static str)] {
        Self::EMBEDDED_PERSONAS
    }

    /// Write persona JSON to the cache directory
    fn cache_persona(persona_id: &str, persona: &Persona) {
        let cache_dir = persona_cache_dir();
        let path = cache_dir.join(format!("{persona_id}.json"));

        match serde_json::to_string_pretty(persona) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to cache persona"
                    );
                } else {
                    tracing::debug!(path = %path.display(), "cached persona");
                }
            }
            Err(e) => {
                tracing::warn!(
                    persona_id,
                    error = %e,
                    "failed to serialize persona for cache"
                );
            }
        }
    }

    /// Load a persona from the cache directory
    fn load_cached_persona(persona_id: &str) -> Result<Persona> {
        let cache_dir = persona_cache_dir();
        Self::load_persona(&cache_dir, persona_id)
    }

    /// Fetch a persona from Manifold registry via web router
    ///
    /// Uses `spawn_blocking` to avoid dropping a `reqwest::blocking::Client`
    /// inside the async runtime, which causes a panic on shutdown
    fn fetch_persona_from_manifold(
        base_url: &str,
        namespace: &str,
        persona_id: &str,
    ) -> Result<Persona> {
        let url = format!(
            "{}/@{}/personas/{}",
            base_url.trim_end_matches('/'),
            namespace,
            persona_id
        );

        let persona_id_owned = persona_id.to_string();
        let namespace_owned = namespace.to_string();

        // Run on a dedicated OS thread so the blocking client's internal
        // runtime is created and dropped outside the async context
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| Error::Config(format!("failed to create HTTP client: {e}")))?;

            let response = client.get(&url).send().map_err(|e| {
                Error::Config(format!("failed to fetch persona from Manifold: {e}"))
            })?;

            if !response.status().is_success() {
                return Err(Error::Config(format!(
                    "persona '{}' not found in namespace '{}' ({})",
                    persona_id_owned, namespace_owned, response.status()
                )));
            }

            let content = response.text().map_err(|e| {
                Error::Config(format!("failed to read Manifold response: {e}"))
            })?;

            let persona: Persona = serde_json::from_str(&content).map_err(|e| {
                Error::Config(format!("failed to parse persona JSON: {e}"))
            })?;

            tracing::info!(
                persona_id = persona_id_owned,
                namespace = namespace_owned,
                "loaded persona from Manifold"
            );

            Ok(persona)
        })
        .join()
        .map_err(|_| Error::Config("persona fetch thread panicked".to_string()))?
    }
}
