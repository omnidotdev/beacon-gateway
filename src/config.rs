//! Configuration management for Beacon gateway

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

    /// Identity service URL for BYOK key resolution
    pub auth_base_url: Option<String>,

    /// Service-to-service key for authenticating to identity service
    pub service_key: Option<String>,

    /// Synapse AI router URL
    pub synapse_url: String,

    /// LLM model identifier for chat completions
    pub llm_model: String,

    /// Cloud mode: requires JWT auth, enables rate limiting
    pub cloud_mode: bool,
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

/// Return the XDG cache directory for persona files, creating it if needed
///
/// Uses `~/.cache/omni/beacon/personas/` on Linux
pub fn persona_cache_dir() -> PathBuf {
    let cache_dir = directories::ProjectDirs::from("dev", "omni", "omni").map_or_else(
        || PathBuf::from(".cache/beacon/personas"),
        |d| d.cache_dir().join("beacon").join("personas"),
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
        // Load persona with priority: env override → Manifold → cache → embedded
        let persona = Self::load_persona_with_priority(persona_id)?;
        let cache_dir = persona_cache_dir();

        // Load API keys from environment
        let api_keys = ApiKeys {
            openai: std::env::var("OPENAI_API_KEY").ok(),
            anthropic: std::env::var("ANTHROPIC_API_KEY").ok(),
            openrouter: std::env::var("OPENROUTER_API_KEY").ok(),
            elevenlabs: std::env::var("ELEVENLABS_API_KEY").ok(),
            deepgram: std::env::var("DEEPGRAM_API_KEY").ok(),
            discord: std::env::var("DISCORD_TOKEN").ok(),
            slack: std::env::var("SLACK_BOT_TOKEN").ok(),
            telegram: std::env::var("TELEGRAM_BOT_TOKEN").ok(),
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

        // API server config
        let api_server = ApiServerConfig {
            port: std::env::var("BEACON_API_PORT")
                .or_else(|_| std::env::var("PORT"))
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(18790),
            api_key: std::env::var("BEACON_API_KEY").ok(),
            manifold_url: std::env::var("MANIFOLD_URL").ok(),
            vortex_url: std::env::var("VORTEX_URL").ok(),
            static_dir: std::env::var("BEACON_STATIC_DIR").ok().map(PathBuf::from),
        };

        // Voice config (routes through Synapse for STT/TTS)
        let tts_voice = persona.tts_voice().unwrap_or("alloy").to_string();
        let tts_speed = f64::from(persona.tts_speed());
        let voice = VoiceConfig {
            enabled: !disable_voice,
            stt_model: std::env::var("BEACON_STT_MODEL")
                .unwrap_or_else(|_| "whisper-1".to_string()),
            tts_model: std::env::var("BEACON_TTS_MODEL")
                .unwrap_or_else(|_| "tts-1".to_string()),
            tts_voice,
            tts_speed,
        };

        if disable_voice {
            tracing::info!("voice explicitly disabled via --disable-voice");
        }

        // Determine data directory (~/.local/share/omni/beacon on Linux)
        let data_dir = directories::ProjectDirs::from("dev", "omni", "omni")
            .map_or_else(|| PathBuf::from("."), |d| d.data_dir().join("beacon"));

        // Ensure data dir exists
        std::fs::create_dir_all(&data_dir).ok();

        // Extension directory (~/.beacon/extensions)
        let extension_dir = std::env::var("BEACON_EXTENSION_DIR").map_or_else(
            |_| {
                directories::BaseDirs::new().map_or_else(
                    || PathBuf::from(".beacon/extensions"),
                    |dirs| dirs.home_dir().join(".beacon/extensions"),
                )
            },
            PathBuf::from,
        );

        // Find life.json for local user
        let life_json_path = Self::find_life_json();

        // LLM provider preference (anthropic or openai)
        let llm_provider = std::env::var("BEACON_LLM_PROVIDER").ok();

        // iMessage config (macOS only)
        let imessage = IMessageConfig {
            enabled: std::env::var("IMESSAGE_ENABLED")
                .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
            cli_path: std::env::var("IMESSAGE_CLI_PATH").ok(),
            db_path: std::env::var("IMESSAGE_DB_PATH").ok(),
            region: std::env::var("IMESSAGE_REGION").ok(),
            service: std::env::var("IMESSAGE_SERVICE").ok(),
        };

        // DM security policy (open, pairing, allowlist)
        let dm_policy = std::env::var("BEACON_DM_POLICY")
            .map(|s| DmPolicy::from_str(&s))
            .unwrap_or_default();

        // Cloud relay configuration
        let relay = RelayConfig::from_env();

        // Gateway authentication configuration
        let auth = AuthConfig::from_env();

        // Hooks configuration (from ~/.beacon/hooks.toml or defaults)
        let hooks = Self::load_hooks_config(&data_dir);

        // Identity service integration for BYOK
        let auth_base_url = std::env::var("AUTH_BASE_URL").ok();
        let service_key = std::env::var("BEACON_SERVICE_KEY").ok();

        // Synapse AI router
        let synapse_url = std::env::var("SYNAPSE_URL")
            .unwrap_or_else(|_| "http://localhost:6000".to_string());
        let llm_model = std::env::var("BEACON_LLM_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

        let cloud_mode = std::env::var("BEACON_CLOUD_MODE")
            .is_ok_and(|v| v == "true" || v == "1");

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
            service_key,
            synapse_url,
            llm_model,
            cloud_mode,
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
        ("orin", include_str!("../personas/orin.json")),
        ("microcap", include_str!("../personas/microcap.json")),
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

    /// Fetch a persona from Manifold registry via web router (blocking)
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

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Config(format!("failed to create HTTP client: {e}")))?;

        let response = client
            .get(&url)
            .send()
            .map_err(|e| Error::Config(format!("failed to fetch persona from Manifold: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Config(format!(
                "persona '{}' not found in namespace '{}' ({})",
                persona_id,
                namespace,
                response.status()
            )));
        }

        let content = response
            .text()
            .map_err(|e| Error::Config(format!("failed to read Manifold response: {e}")))?;

        let persona: Persona = serde_json::from_str(&content)
            .map_err(|e| Error::Config(format!("failed to parse persona JSON: {e}")))?;

        tracing::info!(
            persona_id,
            namespace,
            "loaded persona from Manifold"
        );

        Ok(persona)
    }
}
