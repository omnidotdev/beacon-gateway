//! Interactive first-run setup wizard (`beacon setup`)

use std::path::PathBuf;

use dialoguer::{Confirm, Input, MultiSelect, Select};

use crate::Config;
use crate::config::file::{
    ApiKeysFileConfig, BeaconConfigFile, ChannelToggle, ChannelsFileConfig, LlmFileConfig,
    ServerFileConfig, VoiceFileConfig,
};

/// Run the interactive setup wizard
///
/// # Errors
///
/// Returns error if user input fails or config cannot be written
#[allow(clippy::too_many_lines)]
pub fn run_setup() -> anyhow::Result<()> {
    println!("\n  Beacon Setup\n");
    println!("  Welcome! This wizard will help you configure your");
    println!("  AI assistant. Press Ctrl+C to exit at any time.\n");

    // Load existing config if present
    let existing = crate::config::file::load_config_file();
    let config_path = crate::config::file::config_file_path()
        .unwrap_or_else(|| PathBuf::from("~/.config/omni/beacon/config.toml"));

    if config_path.exists() {
        println!("Existing config found at {}\n", config_path.display());
    }

    println!("--- Persona ---\n");

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

    println!("\n--- LLM Provider ---\n");

    // 2. LLM provider + API key
    let providers = ["Anthropic", "OpenAI", "OpenRouter"];
    let default_provider = existing
        .llm
        .provider
        .as_deref()
        .and_then(|p| providers.iter().position(|&l| l.eq_ignore_ascii_case(p)))
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
        "openrouter" => (
            "OPENROUTER_API_KEY",
            existing.api_keys.openrouter.as_deref(),
        ),
        _ => ("", None),
    };

    let masked = existing_key.map(|k| {
        if k.len() > 8 {
            format!("{}...{}", &k[..4], &k[k.len() - 4..])
        } else {
            "****".to_string()
        }
    });

    let prompt = masked.as_ref().map_or_else(
        || format!("{provider_name} API key ({env_hint})"),
        |m| format!("{provider_name} API key (current: {m}, leave blank to keep)"),
    );

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
            "openai" => "gpt-4o",
            "openrouter" => "anthropic/claude-sonnet-4-20250514",
            // "anthropic" and everything else
            _ => "claude-sonnet-4-20250514",
        });

    let model: String = Input::new()
        .with_prompt("LLM model")
        .default(default_model.to_string())
        .interact_text()?;

    println!("\n--- Voice ---\n");

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
                let key: String = Input::new().with_prompt("OpenAI API key").interact_text()?;
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
                    .clone()
                    .unwrap_or_else(|| "whisper-1".to_string()),
            ),
            tts_model: Some(
                existing
                    .voice
                    .tts_model
                    .clone()
                    .unwrap_or_else(|| "tts-1".to_string()),
            ),
            tts_voice: Some(
                existing
                    .voice
                    .tts_voice
                    .clone()
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

    // 5. Channels
    let channels = setup_channels(&mut api_keys, &existing)?;

    // 6. MCP servers
    let mcp_servers = setup_mcp_servers(&existing.mcp_servers)?;

    // 7. life.json
    let life_json = setup_life_json(existing.life_json.as_deref())?;

    // 8. Build and write config
    let config_file = BeaconConfigFile {
        persona: Some(persona),
        llm: LlmFileConfig {
            model: Some(model),
            provider: Some(provider_name),
        },
        voice,
        api_keys,
        channels,
        server: ServerFileConfig {
            port: existing.server.port,
            synapse_url: existing.server.synapse_url,
            cloud_mode: existing.server.cloud_mode,
        },
        skills: existing.skills,
        mcp_servers,
        life_json,
        ecosystem: existing.ecosystem,
    };

    write_config(&config_path, &config_file)?;
    println!("\nConfig written to {}", config_path.display());

    println!("\n  Setup complete!\n");
    println!("  Start Beacon:");
    println!("    beacon --foreground -v\n");
    println!("  Or install as a service:");
    println!("    beacon install\n");

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
    use std::fmt::Write;

    let mut out = String::new();

    if let Some(ref persona) = config.persona {
        let _ = write!(out, "persona = \"{persona}\"\n\n");
    }

    if let Some(ref path) = config.life_json {
        let _ = writeln!(out, "life_json = \"{path}\"\n");
    }

    // [llm]
    if config.llm.model.is_some() || config.llm.provider.is_some() {
        out.push_str("[llm]\n");
        if let Some(ref model) = config.llm.model {
            let _ = writeln!(out, "model = \"{model}\"");
        }
        if let Some(ref provider) = config.llm.provider {
            let _ = writeln!(out, "provider = \"{provider}\"");
        }
        out.push('\n');
    }

    // [voice]
    if config.voice.enabled.is_some() {
        out.push_str("[voice]\n");
        if let Some(enabled) = config.voice.enabled {
            let _ = writeln!(out, "enabled = {enabled}");
        }
        if let Some(ref m) = config.voice.stt_model {
            let _ = writeln!(out, "stt_model = \"{m}\"");
        }
        if let Some(ref m) = config.voice.tts_model {
            let _ = writeln!(out, "tts_model = \"{m}\"");
        }
        if let Some(ref v) = config.voice.tts_voice {
            let _ = writeln!(out, "tts_voice = \"{v}\"");
        }
        if let Some(s) = config.voice.tts_speed {
            let _ = writeln!(out, "tts_speed = {s}");
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
                let _ = writeln!(out, "{key} = \"{v}\"");
            }
        }
        out.push('\n');
    }

    // [server]
    let sv = &config.server;
    if sv.port.is_some() || sv.synapse_url.is_some() || sv.cloud_mode.is_some() {
        out.push_str("[server]\n");
        if let Some(port) = sv.port {
            let _ = writeln!(out, "port = {port}");
        }
        if let Some(ref url) = sv.synapse_url {
            let _ = writeln!(out, "synapse_url = \"{url}\"");
        }
        if let Some(cloud) = sv.cloud_mode {
            let _ = writeln!(out, "cloud_mode = {cloud}");
        }
        out.push('\n');
    }

    // [channels]
    let ch = &config.channels;
    let has_channels = ch.discord.is_some() || ch.telegram.is_some() || ch.slack.is_some();
    if has_channels {
        out.push_str("[channels]\n\n");
        if let Some(ref d) = ch.discord {
            out.push_str("[channels.discord]\n");
            if let Some(enabled) = d.enabled {
                let _ = writeln!(out, "enabled = {enabled}");
            }
            out.push('\n');
        }
        if let Some(ref t) = ch.telegram {
            out.push_str("[channels.telegram]\n");
            if let Some(enabled) = t.enabled {
                let _ = writeln!(out, "enabled = {enabled}");
            }
            out.push('\n');
        }
        if let Some(ref s) = ch.slack {
            out.push_str("[channels.slack]\n");
            if let Some(enabled) = s.enabled {
                let _ = writeln!(out, "enabled = {enabled}");
            }
            out.push('\n');
        }
    }

    // [[mcp_servers]]
    for server in &config.mcp_servers {
        out.push_str("[[mcp_servers]]\n");
        let _ = writeln!(out, "name = \"{}\"", server.name);
        let _ = writeln!(out, "command = \"{}\"", server.command);
        if !server.args.is_empty() {
            let args_str: Vec<String> = server.args.iter().map(|a| format!("\"{a}\"")).collect();
            let _ = writeln!(out, "args = [{}]", args_str.join(", "));
        }
        if !server.env.is_empty() {
            out.push_str("[mcp_servers.env]\n");
            for (k, v) in &server.env {
                let _ = writeln!(out, "{k} = \"{v}\"");
            }
        }
        out.push('\n');
    }

    out
}

/// Prompt user to configure messaging channels
fn setup_channels(
    api_keys: &mut ApiKeysFileConfig,
    existing: &BeaconConfigFile,
) -> anyhow::Result<ChannelsFileConfig> {
    println!("\n--- Channel Setup ---\n");

    let channels = ["Discord", "Telegram", "Slack", "Skip (configure later)"];
    let defaults: Vec<bool> = channels
        .iter()
        .map(|c| match *c {
            "Discord" => existing
                .channels
                .discord
                .as_ref()
                .and_then(|d| d.enabled)
                .unwrap_or(false),
            "Telegram" => existing
                .channels
                .telegram
                .as_ref()
                .and_then(|t| t.enabled)
                .unwrap_or(false),
            "Slack" => existing
                .channels
                .slack
                .as_ref()
                .and_then(|s| s.enabled)
                .unwrap_or(false),
            _ => false,
        })
        .collect();

    let selected = MultiSelect::new()
        .with_prompt("Which channels do you want to connect? (space to toggle, enter to confirm)")
        .items(&channels)
        .defaults(&defaults)
        .interact()?;

    // If "Skip" selected or nothing selected, preserve existing config
    if selected.is_empty() || selected.contains(&3) {
        return Ok(existing.channels.clone());
    }

    let mut channels_config = existing.channels.clone();

    for &idx in &selected {
        match channels[idx] {
            "Discord" => {
                let token = prompt_channel_token("Discord bot token", api_keys.discord.as_deref())?;
                if let Some(t) = token {
                    api_keys.discord = Some(t);
                }
                channels_config.discord = Some(ChannelToggle {
                    enabled: Some(true),
                });
            }
            "Telegram" => {
                let token =
                    prompt_channel_token("Telegram bot token (from @BotFather)", api_keys.telegram.as_deref())?;
                if let Some(t) = token {
                    api_keys.telegram = Some(t);
                }
                channels_config.telegram = Some(ChannelToggle {
                    enabled: Some(true),
                });
            }
            "Slack" => {
                let token =
                    prompt_channel_token("Slack bot token (xoxb-...)", api_keys.slack.as_deref())?;
                if let Some(t) = token {
                    api_keys.slack = Some(t);
                }
                channels_config.slack = Some(ChannelToggle {
                    enabled: Some(true),
                });
            }
            _ => {}
        }
    }

    Ok(channels_config)
}

/// Prompt for a channel token with masked existing value
fn prompt_channel_token(
    label: &str,
    existing: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let masked = existing.map(|k| {
        if k.len() > 8 {
            format!("{}...{}", &k[..4], &k[k.len() - 4..])
        } else {
            "****".to_string()
        }
    });

    let prompt = masked.as_ref().map_or_else(
        || label.to_string(),
        |m| format!("{label} (current: {m}, leave blank to keep)"),
    );

    let input: String = Input::new()
        .with_prompt(&prompt)
        .allow_empty(true)
        .interact_text()?;

    if input.is_empty() {
        Ok(existing.map(str::to_string))
    } else {
        Ok(Some(input))
    }
}

/// Scan PATH for well-known MCP servers and offer to enable them
fn setup_mcp_servers(
    existing: &[crate::mcp::McpServerConfig],
) -> anyhow::Result<Vec<crate::mcp::McpServerConfig>> {
    println!("\n--- MCP Server Discovery ---\n");

    // Well-known MCP servers with their binary names and typical commands
    let known_servers: &[(&str, &str, &[&str])] = &[
        ("filesystem", "npx", &["-y", "@modelcontextprotocol/server-filesystem", "."]),
        ("github", "npx", &["-y", "@modelcontextprotocol/server-github"]),
        ("postgres", "npx", &["-y", "@modelcontextprotocol/server-postgres"]),
        ("sqlite", "npx", &["-y", "@modelcontextprotocol/server-sqlite"]),
        ("brave-search", "npx", &["-y", "@modelcontextprotocol/server-brave-search"]),
        ("fetch", "npx", &["-y", "@modelcontextprotocol/server-fetch"]),
        ("memory", "npx", &["-y", "@modelcontextprotocol/server-memory"]),
    ];

    // Check which are already configured
    let existing_names: Vec<&str> = existing.iter().map(|s| s.name.as_str()).collect();

    let available: Vec<&(&str, &str, &[&str])> = known_servers
        .iter()
        .filter(|(name, _, _)| !existing_names.contains(name))
        .collect();

    if available.is_empty() && !existing.is_empty() {
        println!(
            "{} MCP server(s) already configured. Skipping discovery.",
            existing.len()
        );
        return Ok(existing.to_vec());
    }

    if available.is_empty() {
        let add_custom = Confirm::new()
            .with_prompt("No well-known MCP servers found to add. Add a custom one?")
            .default(false)
            .interact()?;

        if !add_custom {
            return Ok(existing.to_vec());
        }

        let mut servers = existing.to_vec();
        if let Some(custom) = prompt_custom_mcp_server()? {
            servers.push(custom);
        }
        return Ok(servers);
    }

    let labels: Vec<String> = available
        .iter()
        .map(|(name, _, _)| name.to_string())
        .collect();

    let selected = MultiSelect::new()
        .with_prompt("Enable MCP servers (space to toggle, enter to confirm)")
        .items(&labels)
        .interact()?;

    let mut servers = existing.to_vec();

    for idx in selected {
        let (name, command, args) = available[idx];
        servers.push(crate::mcp::McpServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: std::collections::HashMap::new(),
        });
        println!("  + Added {name}");
    }

    // Offer to add custom server
    let add_custom = Confirm::new()
        .with_prompt("Add a custom MCP server?")
        .default(false)
        .interact()?;

    if add_custom {
        if let Some(custom) = prompt_custom_mcp_server()? {
            println!("  + Added {}", custom.name);
            servers.push(custom);
        }
    }

    Ok(servers)
}

/// Prompt for a custom MCP server definition
fn prompt_custom_mcp_server() -> anyhow::Result<Option<crate::mcp::McpServerConfig>> {
    let name: String = Input::new()
        .with_prompt("Server name")
        .interact_text()?;

    if name.is_empty() {
        return Ok(None);
    }

    let command: String = Input::new()
        .with_prompt("Command (e.g. npx, python, node)")
        .interact_text()?;

    let args_str: String = Input::new()
        .with_prompt("Arguments (space-separated)")
        .allow_empty(true)
        .interact_text()?;

    let args: Vec<String> = if args_str.is_empty() {
        vec![]
    } else {
        args_str.split_whitespace().map(str::to_string).collect()
    };

    Ok(Some(crate::mcp::McpServerConfig {
        name,
        command,
        args,
        env: std::collections::HashMap::new(),
    }))
}

/// Prompt for life.json path or URL
fn setup_life_json(existing: Option<&str>) -> anyhow::Result<Option<String>> {
    println!("\n--- life.json (Optional) ---\n");
    println!("life.json is a portable identity file that gives your assistant");
    println!("context about you (name, timezone, preferences, etc.).\n");

    let configure = Confirm::new()
        .with_prompt("Set up a life.json file?")
        .default(existing.is_some())
        .interact()?;

    if !configure {
        return Ok(existing.map(str::to_string));
    }

    let default_path = existing
        .map(str::to_string)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
            format!("{home}/.life.json")
        });

    let path: String = Input::new()
        .with_prompt("Path or URL to life.json")
        .default(default_path)
        .interact_text()?;

    if path.is_empty() {
        Ok(existing.map(str::to_string))
    } else {
        Ok(Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::file::*;

    #[test]
    fn serialize_config_includes_persona() {
        let config = BeaconConfigFile {
            persona: Some("orin".to_string()),
            ..Default::default()
        };
        let toml = serialize_config(&config);
        assert!(toml.contains("persona = \"orin\""));
    }

    #[test]
    fn serialize_config_includes_channels() {
        let config = BeaconConfigFile {
            channels: ChannelsFileConfig {
                discord: Some(ChannelToggle {
                    enabled: Some(true),
                }),
                telegram: Some(ChannelToggle {
                    enabled: Some(false),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let toml = serialize_config(&config);
        assert!(toml.contains("[channels.discord]"));
        assert!(toml.contains("enabled = true"));
        assert!(toml.contains("[channels.telegram]"));
        assert!(toml.contains("enabled = false"));
    }

    #[test]
    fn serialize_config_includes_mcp_servers() {
        let config = BeaconConfigFile {
            mcp_servers: vec![crate::mcp::McpServerConfig {
                name: "github".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()],
                env: std::collections::HashMap::new(),
            }],
            ..Default::default()
        };
        let toml = serialize_config(&config);
        assert!(toml.contains("[[mcp_servers]]"));
        assert!(toml.contains("name = \"github\""));
        assert!(toml.contains("command = \"npx\""));
    }

    #[test]
    fn serialize_config_includes_life_json() {
        let config = BeaconConfigFile {
            life_json: Some("/home/user/.life.json".to_string()),
            ..Default::default()
        };
        let toml = serialize_config(&config);
        assert!(toml.contains("life_json = \"/home/user/.life.json\""));
    }

    #[test]
    fn serialize_config_omits_empty_sections() {
        let config = BeaconConfigFile::default();
        let toml = serialize_config(&config);
        assert!(!toml.contains("[channels]"));
        assert!(!toml.contains("[[mcp_servers]]"));
        assert!(!toml.contains("life_json"));
    }

    #[test]
    fn serialize_config_full_roundtrip() {
        let config = BeaconConfigFile {
            persona: Some("orin".to_string()),
            llm: LlmFileConfig {
                model: Some("claude-sonnet-4-20250514".to_string()),
                provider: Some("anthropic".to_string()),
            },
            voice: VoiceFileConfig {
                enabled: Some(true),
                stt_model: Some("whisper-1".to_string()),
                tts_model: Some("tts-1".to_string()),
                tts_voice: Some("nova".to_string()),
                tts_speed: Some(1.0),
            },
            api_keys: ApiKeysFileConfig {
                anthropic: Some("sk-ant-test".to_string()),
                ..Default::default()
            },
            channels: ChannelsFileConfig {
                discord: Some(ChannelToggle { enabled: Some(true) }),
                ..Default::default()
            },
            mcp_servers: vec![crate::mcp::McpServerConfig {
                name: "test".to_string(),
                command: "echo".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            }],
            life_json: Some("~/.life.json".to_string()),
            ..Default::default()
        };

        let toml = serialize_config(&config);

        // Verify all sections present
        assert!(toml.contains("persona = \"orin\""));
        assert!(toml.contains("[llm]"));
        assert!(toml.contains("[voice]"));
        assert!(toml.contains("[api_keys]"));
        assert!(toml.contains("[channels.discord]"));
        assert!(toml.contains("[[mcp_servers]]"));
        assert!(toml.contains("life_json"));
    }
}
