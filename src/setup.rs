//! Interactive first-run setup wizard (`beacon setup`)

use std::path::PathBuf;

use dialoguer::{Confirm, Input, Select};

use crate::config::file::{
    ApiKeysFileConfig, BeaconConfigFile, LlmFileConfig, ServerFileConfig, VoiceFileConfig,
};
use crate::Config;

/// Run the interactive setup wizard
///
/// # Errors
///
/// Returns error if user input fails or config cannot be written
pub fn run_setup() -> anyhow::Result<()> {
    println!("Beacon Setup\n");

    // Load existing config if present
    let existing = crate::config::file::load_config_file();
    let config_path = crate::config::file::config_file_path()
        .unwrap_or_else(|| PathBuf::from("~/.config/omni/beacon/config.toml"));

    if config_path.exists() {
        println!("Existing config found at {}\n", config_path.display());
    }

    // 1. Persona selection
    let personas = Config::embedded_personas();
    let mut persona_labels: Vec<&str> = vec!["(none)"];
    persona_labels.extend(personas.iter().map(|(id, _)| *id));

    let default_persona = existing
        .persona
        .as_deref()
        .and_then(|p| {
            if p.is_empty() {
                Some(0) // "(none)" is at index 0
            } else {
                persona_labels.iter().position(|&l| l == p)
            }
        })
        .unwrap_or(1); // Default to first real persona

    let persona_idx = Select::new()
        .with_prompt("Select a persona")
        .items(&persona_labels)
        .default(default_persona)
        .interact()?;
    let persona = if persona_idx == 0 {
        String::new() // "(none)" selected
    } else {
        persona_labels[persona_idx].to_string()
    };

    // 2. LLM provider + API key
    let providers = ["Anthropic", "OpenAI", "OpenRouter"];
    let default_provider = existing
        .llm
        .provider
        .as_deref()
        .and_then(|p| {
            providers
                .iter()
                .position(|&l| l.eq_ignore_ascii_case(p))
        })
        .unwrap_or(0);

    let provider_idx = Select::new()
        .with_prompt("Select an LLM provider")
        .items(&providers)
        .default(default_provider)
        .interact()?;
    let provider_name = providers[provider_idx].to_lowercase();

    let (env_hint, existing_key) = match provider_name.as_str() {
        "anthropic" => ("ANTHROPIC_API_KEY", existing.api_keys.anthropic.as_deref()),
        "openai" => ("OPENAI_API_KEY", existing.api_keys.openai.as_deref()),
        "openrouter" => ("OPENROUTER_API_KEY", existing.api_keys.openrouter.as_deref()),
        _ => ("", None),
    };

    let masked = existing_key.map(|k| {
        if k.len() > 8 {
            format!("{}...{}", &k[..4], &k[k.len() - 4..])
        } else {
            "****".to_string()
        }
    });

    let prompt = if let Some(ref m) = masked {
        format!("{provider_name} API key (current: {m}, leave blank to keep)")
    } else {
        format!("{provider_name} API key ({env_hint})")
    };

    let api_key_input: String = Input::new()
        .with_prompt(&prompt)
        .allow_empty(true)
        .interact_text()?;

    let api_key = if api_key_input.is_empty() {
        existing_key.map(str::to_string)
    } else {
        Some(api_key_input)
    };

    // Build API keys config
    let mut api_keys = ApiKeysFileConfig::default();
    match provider_name.as_str() {
        "anthropic" => api_keys.anthropic = api_key,
        "openai" => api_keys.openai = api_key,
        "openrouter" => api_keys.openrouter = api_key,
        _ => {}
    }

    // 3. LLM model
    let default_model = existing
        .llm
        .model
        .as_deref()
        .unwrap_or(match provider_name.as_str() {
            "anthropic" => "claude-sonnet-4-20250514",
            "openai" => "gpt-4o",
            "openrouter" => "anthropic/claude-sonnet-4-20250514",
            _ => "claude-sonnet-4-20250514",
        });

    let model: String = Input::new()
        .with_prompt("LLM model")
        .default(default_model.to_string())
        .interact_text()?;

    // 4. Voice (optional)
    let voice_default = existing.voice.enabled.unwrap_or(true);
    let enable_voice = Confirm::new()
        .with_prompt("Enable voice (STT/TTS)?")
        .default(voice_default)
        .interact()?;

    let voice = if enable_voice {
        // If using OpenAI for voice and no key yet, ask for it
        if api_keys.openai.is_none() && provider_name != "openai" {
            let need_openai = Confirm::new()
                .with_prompt("Voice requires an OpenAI key for Whisper/TTS. Add one?")
                .default(true)
                .interact()?;

            if need_openai {
                let key: String = Input::new()
                    .with_prompt("OpenAI API key")
                    .interact_text()?;
                if !key.is_empty() {
                    api_keys.openai = Some(key);
                }
            }
        }

        VoiceFileConfig {
            enabled: Some(true),
            stt_model: Some(
                existing
                    .voice
                    .stt_model
                    .unwrap_or_else(|| "whisper-1".to_string()),
            ),
            tts_model: Some(
                existing
                    .voice
                    .tts_model
                    .unwrap_or_else(|| "tts-1".to_string()),
            ),
            tts_voice: Some(
                existing
                    .voice
                    .tts_voice
                    .unwrap_or_else(|| "alloy".to_string()),
            ),
            tts_speed: existing.voice.tts_speed.or(Some(1.0)),
        }
    } else {
        VoiceFileConfig {
            enabled: Some(false),
            ..VoiceFileConfig::default()
        }
    };

    // 5. Build and write config
    let config_file = BeaconConfigFile {
        persona: Some(persona),
        llm: LlmFileConfig {
            model: Some(model),
            provider: Some(provider_name),
        },
        voice,
        api_keys,
        channels: existing.channels,
        server: ServerFileConfig {
            port: existing.server.port,
            synapse_url: existing.server.synapse_url,
            cloud_mode: existing.server.cloud_mode,
        },
        skills: existing.skills,
    };

    write_config(&config_path, &config_file)?;
    println!("\nConfig written to {}", config_path.display());

    // 6. Daemon install (optional)
    let install_service = Confirm::new()
        .with_prompt("Install beacon as a system service?")
        .default(false)
        .interact()?;

    if install_service {
        match crate::lifecycle::install_service(&crate::lifecycle::ServiceConfig::default()) {
            Ok(()) => println!("Service installed"),
            Err(e) => println!("Failed to install service: {e}"),
        }
    }

    println!("\nSetup complete! Run `beacon --foreground -v` to start.");

    Ok(())
}

/// Serialize and write the config file
fn write_config(path: &PathBuf, config: &BeaconConfigFile) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let toml = serialize_config(config);
    std::fs::write(path, toml)?;

    Ok(())
}

/// Serialize config to a readable TOML string
fn serialize_config(config: &BeaconConfigFile) -> String {
    let mut out = String::new();

    if let Some(ref persona) = config.persona {
        out.push_str(&format!("persona = \"{persona}\"\n\n"));
    }

    // [llm]
    if config.llm.model.is_some() || config.llm.provider.is_some() {
        out.push_str("[llm]\n");
        if let Some(ref model) = config.llm.model {
            out.push_str(&format!("model = \"{model}\"\n"));
        }
        if let Some(ref provider) = config.llm.provider {
            out.push_str(&format!("provider = \"{provider}\"\n"));
        }
        out.push('\n');
    }

    // [voice]
    if config.voice.enabled.is_some() {
        out.push_str("[voice]\n");
        if let Some(enabled) = config.voice.enabled {
            out.push_str(&format!("enabled = {enabled}\n"));
        }
        if let Some(ref m) = config.voice.stt_model {
            out.push_str(&format!("stt_model = \"{m}\"\n"));
        }
        if let Some(ref m) = config.voice.tts_model {
            out.push_str(&format!("tts_model = \"{m}\"\n"));
        }
        if let Some(ref v) = config.voice.tts_voice {
            out.push_str(&format!("tts_voice = \"{v}\"\n"));
        }
        if let Some(s) = config.voice.tts_speed {
            out.push_str(&format!("tts_speed = {s}\n"));
        }
        out.push('\n');
    }

    // [api_keys]
    let ak = &config.api_keys;
    if ak.anthropic.is_some()
        || ak.openai.is_some()
        || ak.openrouter.is_some()
        || ak.elevenlabs.is_some()
        || ak.deepgram.is_some()
        || ak.discord.is_some()
        || ak.slack.is_some()
        || ak.telegram.is_some()
    {
        out.push_str("[api_keys]\n");
        for (key, val) in [
            ("anthropic", &ak.anthropic),
            ("openai", &ak.openai),
            ("openrouter", &ak.openrouter),
            ("elevenlabs", &ak.elevenlabs),
            ("deepgram", &ak.deepgram),
            ("discord", &ak.discord),
            ("slack", &ak.slack),
            ("telegram", &ak.telegram),
        ] {
            if let Some(v) = val {
                out.push_str(&format!("{key} = \"{v}\"\n"));
            }
        }
        out.push('\n');
    }

    // [server]
    let sv = &config.server;
    if sv.port.is_some() || sv.synapse_url.is_some() || sv.cloud_mode.is_some() {
        out.push_str("[server]\n");
        if let Some(port) = sv.port {
            out.push_str(&format!("port = {port}\n"));
        }
        if let Some(ref url) = sv.synapse_url {
            out.push_str(&format!("synapse_url = \"{url}\"\n"));
        }
        if let Some(cloud) = sv.cloud_mode {
            out.push_str(&format!("cloud_mode = {cloud}\n"));
        }
        out.push('\n');
    }

    out
}
