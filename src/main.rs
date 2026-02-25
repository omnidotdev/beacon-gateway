use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use beacon_gateway::db::{self, UserRepo};
use beacon_gateway::voice::{AudioCapture, AudioPlayback};
use beacon_gateway::{Config, Daemon};

/// Beacon - Voice and messaging gateway for AI assistants
#[derive(Parser)]
#[command(name = "beacon", version, about)]
struct Cli {
    /// Persona to use (e.g., "orin"); omit or pass "" for no persona
    #[arg(short, long, env = "BEACON_PERSONA")]
    persona: Option<String>,

    /// Port to listen on
    #[arg(long, env = "BEACON_PORT", default_value = "18789")]
    port: u16,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Run in foreground (don't daemonize)
    #[arg(long)]
    foreground: bool,

    /// Disable voice features (for headless servers without audio hardware)
    #[arg(long, env = "BEACON_DISABLE_VOICE")]
    disable_voice: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Command {
    /// Test microphone input
    TestMic {
        /// Duration in seconds
        #[arg(short, long, default_value = "5")]
        duration: u64,
    },
    /// Test speaker output
    TestSpeaker,
    /// Test TTS output
    TestTts {
        /// Text to speak
        #[arg(default_value = "Hello! This is a test of the text to speech system.")]
        text: String,
    },
    /// Set a user's life.json path
    SetLifeJson {
        /// User ID (platform-specific, e.g., Discord user ID)
        #[arg(short, long)]
        user: String,
        /// Path to life.json file (local path or URL)
        path: String,
    },
    /// Show a user's current life.json path
    GetLifeJson {
        /// User ID
        #[arg(short, long)]
        user: String,
    },
    /// Install beacon as a system service
    Install,
    /// Uninstall the beacon system service
    Uninstall,
    /// Show service status
    Status,
    /// Tail the service log file
    Logs {
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    /// Interactive first-run setup
    Setup,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Set up logging based on verbosity
    let filter = match cli.verbose {
        0 => "info,beacon_gateway=info",
        1 => "info,beacon_gateway=debug",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .init();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::future_not_send)]
async fn run(cli: Cli) -> anyhow::Result<()> {
    let persona_ref = cli.persona.as_deref();

    // Handle subcommands
    if let Some(cmd) = cli.command {
        return match cmd {
            Command::TestMic { duration } => test_mic(duration).await,
            Command::TestSpeaker => test_speaker().await,
            Command::TestTts { text } => test_tts(persona_ref, &text).await,
            Command::SetLifeJson { user, path } => set_life_json(persona_ref, &user, &path),
            Command::GetLifeJson { user } => get_life_json(persona_ref, &user),
            Command::Install => cmd_install(persona_ref, cli.port),
            Command::Uninstall => cmd_uninstall(),
            Command::Status => cmd_status(),
            Command::Logs { lines, follow } => cmd_logs(lines, follow),
            Command::Setup => beacon_gateway::setup::run_setup(),
        };
    }

    tracing::info!(
        persona = ?cli.persona,
        port = cli.port,
        disable_voice = cli.disable_voice,
        "starting beacon gateway"
    );

    // Load configuration
    let config = Config::load_with_options(persona_ref, cli.disable_voice)?;
    tracing::debug!(?config, "loaded configuration");

    let voice_enabled = config.voice.enabled;
    let wake_word = config.persona.wake_word().map(ToString::to_string);

    // Create and run daemon
    let daemon = Daemon::new(config, cli.port).await?;

    if voice_enabled {
        if let Some(ww) = &wake_word {
            tracing::info!("beacon gateway ready - say \"{ww}\"");
        } else {
            tracing::info!("beacon gateway ready (no wake word configured)");
        }
    } else {
        tracing::info!("beacon gateway ready (messaging-only mode, voice disabled)");
    }

    // Run until interrupted
    daemon.run().await?;

    Ok(())
}

/// Test microphone input
#[allow(clippy::future_not_send)]
async fn test_mic(duration: u64) -> anyhow::Result<()> {
    println!("Testing microphone for {duration} seconds...");
    println!("Speak into your microphone!\n");

    let mut capture = AudioCapture::new()?;
    capture.start()?;

    let sample_rate = capture.sample_rate();
    println!("Sample rate: {sample_rate} Hz");
    println!("---");

    for i in 0..duration {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let samples = capture.peek_buffer();
        let energy = calculate_rms(&samples);
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Visual meter
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let meter_len = (energy * 100.0).min(50.0) as usize;
        let meter: String = "█".repeat(meter_len) + &" ".repeat(50 - meter_len);

        println!(
            "[{:2}s] RMS: {:.4} | Peak: {:.4} | [{}]",
            i + 1,
            energy,
            peak,
            meter
        );

        // Clear buffer each second
        capture.clear_buffer();
    }

    capture.stop();

    println!("\n---");
    println!("If you saw movement in the meter, your mic is working!");
    println!("If RMS stayed near 0, check:");
    println!("  1. Is your mic plugged in?");
    println!("  2. Run: pactl info | grep 'Default Source'");
    println!("  3. Run: arecord -l (to list devices)");
    println!("  4. Try: pavucontrol (to check levels)");

    Ok(())
}

/// Calculate RMS energy
#[allow(clippy::cast_precision_loss)]
fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

/// Test speaker output with a sine wave
async fn test_speaker() -> anyhow::Result<()> {
    println!("Testing speaker output...");
    println!("You should hear a 440Hz tone for 2 seconds\n");

    let mut playback = AudioPlayback::new()?;

    // Generate 2 seconds of 440Hz sine wave at 24kHz sample rate
    let sample_rate = 24000_i32;
    let frequency = 440.0_f32;
    let duration_secs = 2.0_f32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let num_samples = (sample_rate as f32 * duration_secs) as usize;

    #[allow(clippy::cast_precision_loss)]
    let samples: Vec<f32> = (0..num_samples)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            (2.0 * std::f32::consts::PI * frequency * t).sin() * 0.3 // 30% volume
        })
        .collect();

    println!("Playing {} samples at {} Hz...", samples.len(), sample_rate);

    playback.play(samples).await?;

    println!("\n---");
    println!("If you heard the tone, your speakers are working!");
    println!("If you didn't hear anything, check:");
    println!("  1. Run: pactl info | grep 'Default Sink'");
    println!("  2. Run: pactl list sinks short");
    println!("  3. Try: pavucontrol (to check output levels)");

    Ok(())
}

/// Test TTS output via Synapse
async fn test_tts(persona: Option<&str>, text: &str) -> anyhow::Result<()> {
    println!("Testing TTS with text: \"{text}\"\n");

    let config = Config::load(persona)?;

    #[cfg(feature = "embedded-synapse")]
    let synapse = if !config.cloud_mode {
        let synapse_cfg =
            beacon_gateway::config::synapse_bridge::build_synapse_config(&config);
        synapse_client::SynapseClient::embedded(synapse_cfg)
            .await
            .map_err(|e| anyhow::anyhow!("embedded synapse init failed: {e}"))?
    } else {
        synapse_client::SynapseClient::new(&config.synapse_url)
            .map_err(|e| anyhow::anyhow!("failed to create Synapse client: {e}"))?
    };

    #[cfg(not(feature = "embedded-synapse"))]
    let synapse = synapse_client::SynapseClient::new(&config.synapse_url)
        .map_err(|e| anyhow::anyhow!("failed to create Synapse client: {e}"))?;

    let request = synapse_client::SpeechRequest {
        model: config.voice.tts_model.clone(),
        input: text.to_string(),
        voice: config.voice.tts_voice.clone(),
        response_format: None,
        speed: Some(config.voice.tts_speed),
    };

    println!("Synthesizing speech...");
    let mp3_data = synapse
        .synthesize(&request)
        .await
        .map_err(|e| anyhow::anyhow!("TTS synthesis failed: {e}"))?;
    println!("Got {} bytes of audio data", mp3_data.len());

    // Check MP3 header
    if mp3_data.len() > 3 {
        println!(
            "First 4 bytes: {:02x} {:02x} {:02x} {:02x}",
            mp3_data[0], mp3_data[1], mp3_data[2], mp3_data[3]
        );
    }

    println!("Playing audio...");
    let mut playback = AudioPlayback::new()?;
    playback.play_mp3(&mp3_data).await?;

    println!("\n---");
    println!("If you heard the speech, TTS is working!");

    Ok(())
}

/// Set a user's life.json path
fn set_life_json(persona: Option<&str>, user_id: &str, path: &str) -> anyhow::Result<()> {
    let config = Config::load(persona)?;
    let db_path = config.data_dir.join("beacon.db");
    let pool = db::init(&db_path)?;
    let user_repo = UserRepo::new(pool);

    // Ensure user exists
    let user = user_repo.find_or_create(user_id)?;

    // Set the path
    let path_to_set = if path.is_empty() || path == "none" {
        None
    } else {
        Some(path)
    };

    user_repo.set_life_json_path(&user.id, path_to_set)?;

    match path_to_set {
        Some(p) => println!("Set life.json path for user {user_id}: {p}"),
        None => println!("Cleared life.json path for user {user_id}"),
    }

    Ok(())
}

/// Get a user's life.json path
fn get_life_json(persona: Option<&str>, user_id: &str) -> anyhow::Result<()> {
    let config = Config::load(persona)?;
    let db_path = config.data_dir.join("beacon.db");
    let pool = db::init(&db_path)?;
    let user_repo = UserRepo::new(pool);

    match user_repo.find_or_create(user_id) {
        Ok(user) => match user.life_json_path {
            Some(path) => println!("User {user_id} life.json: {path}"),
            None => println!("User {user_id} has no life.json configured"),
        },
        Err(e) => println!("Error finding user: {e}"),
    }

    Ok(())
}

/// Install beacon as a system service
fn cmd_install(persona: Option<&str>, port: u16) -> anyhow::Result<()> {
    let binary = std::env::current_exe()?;
    let config = beacon_gateway::lifecycle::ServiceConfig {
        binary_path: binary,
        persona: persona.unwrap_or_default().to_string(),
        port,
        extra_args: Vec::new(),
    };

    beacon_gateway::lifecycle::install_service(&config)?;
    println!("Beacon installed as system service");
    Ok(())
}

/// Uninstall the beacon system service
fn cmd_uninstall() -> anyhow::Result<()> {
    beacon_gateway::lifecycle::uninstall_service()?;
    println!("Beacon system service removed");
    Ok(())
}

/// Show service status
fn cmd_status() -> anyhow::Result<()> {
    let status = beacon_gateway::lifecycle::service_status()?;
    println!("Beacon service: {status}");
    Ok(())
}

/// Tail the service log file
fn cmd_logs(lines: usize, follow: bool) -> anyhow::Result<()> {
    let log_path = beacon_gateway::lifecycle::log_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine log path"))?;

    if !log_path.exists() {
        anyhow::bail!("log file not found: {}", log_path.display());
    }

    let mut args = vec![
        format!("-n{lines}"),
        log_path.display().to_string(),
    ];
    if follow {
        args.insert(0, "-f".to_string());
    }

    let status = std::process::Command::new("tail")
        .args(&args)
        .status()?;

    if !status.success() {
        anyhow::bail!("tail exited with {status}");
    }

    Ok(())
}
