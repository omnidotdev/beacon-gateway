//! Cloud relay configuration for optional remote access
//!
//! Supports multiple relay modes:
//! - Tailscale Serve: tailnet-only access using Tailscale identity
//! - Tailscale Funnel: public HTTPS access
//! - SSH tunnel: reverse tunnel to a remote host

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};

use crate::{Error, Result};

/// Default local port Beacon listens on
const DEFAULT_LOCAL_PORT: u16 = 18790;

/// Cloud relay configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Enable cloud relay
    pub enabled: bool,
    /// Relay mode
    pub mode: RelayMode,
    /// Local port Beacon is listening on
    pub local_port: u16,
}

/// Relay mode options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayMode {
    /// No relay (local only)
    #[default]
    None,
    /// Tailscale Serve (tailnet only, uses Tailscale identity)
    TailscaleServe {
        /// Port to serve on tailnet
        port: u16,
    },
    /// Tailscale Funnel (public HTTPS)
    TailscaleFunnel {
        /// Port to funnel
        port: u16,
        /// Require password for public access
        password: Option<String>,
    },
    /// SSH tunnel to remote host
    SshTunnel {
        /// Remote host
        host: String,
        /// Remote port
        port: u16,
        /// SSH key path
        key_path: Option<PathBuf>,
        /// SSH user
        user: Option<String>,
    },
}

impl RelayConfig {
    /// Load relay configuration from environment variables
    ///
    /// Reads from:
    /// - `BEACON_RELAY_ENABLED`: enable relay (default: false)
    /// - `BEACON_RELAY_MODE`: `tailscale_serve`, `tailscale_funnel`, `ssh_tunnel`
    /// - `BEACON_RELAY_PORT`: port for `Tailscale` modes
    /// - `BEACON_RELAY_PASSWORD`: optional password for funnel mode
    /// - `BEACON_SSH_HOST`: SSH tunnel remote host
    /// - `BEACON_SSH_PORT`: SSH tunnel remote port
    /// - `BEACON_SSH_KEY`: path to SSH private key
    /// - `BEACON_SSH_USER`: SSH username
    /// - `BEACON_PORT`: local port Beacon listens on (default: 18790)
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = std::env::var("BEACON_RELAY_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let local_port = std::env::var("BEACON_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_LOCAL_PORT);

        if !enabled {
            return Self {
                local_port,
                ..Self::default()
            };
        }

        let mode = std::env::var("BEACON_RELAY_MODE")
            .ok()
            .map(|m| Self::parse_mode(&m))
            .unwrap_or_default();

        Self {
            enabled,
            mode,
            local_port,
        }
    }

    /// Parse relay mode from string
    fn parse_mode(mode: &str) -> RelayMode {
        let port = std::env::var("BEACON_RELAY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(443);

        match mode.to_lowercase().as_str() {
            "tailscale_serve" | "tailscaleserve" => RelayMode::TailscaleServe { port },
            "tailscale_funnel" | "tailscalefunnel" => RelayMode::TailscaleFunnel {
                port,
                password: std::env::var("BEACON_RELAY_PASSWORD").ok(),
            },
            "ssh_tunnel" | "sshtunnel" | "ssh" => RelayMode::SshTunnel {
                host: std::env::var("BEACON_SSH_HOST").unwrap_or_default(),
                port: std::env::var("BEACON_SSH_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(22),
                key_path: std::env::var("BEACON_SSH_KEY").ok().map(PathBuf::from),
                user: std::env::var("BEACON_SSH_USER").ok(),
            },
            _ => RelayMode::None,
        }
    }
}

/// Relay status information
#[derive(Debug, Clone, Serialize)]
pub struct RelayStatus {
    /// Whether relay is enabled
    pub enabled: bool,
    /// Current relay mode name
    pub mode: String,
    /// Public URL if available
    pub url: Option<String>,
    /// Whether relay is currently connected
    pub connected: bool,
}

impl Default for RelayStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "none".to_string(),
            url: None,
            connected: false,
        }
    }
}

/// Find a binary on `PATH`
///
/// # Errors
///
/// Returns error if the binary is not found
fn find_binary(name: &str) -> Result<PathBuf> {
    which::which(name).map_err(|_| Error::Config(format!("`{name}` binary not found on PATH")))
}

/// Resolve the public URL for a Tailscale node
///
/// Runs `tailscale status --json` and extracts `Self.DNSName`
async fn resolve_tailscale_url(tailscale_path: &PathBuf, port: u16) -> Option<String> {
    let output = Command::new(tailscale_path)
        .args(["status", "--json"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        tracing::warn!("failed to get Tailscale status");
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let dns_name = json
        .get("Self")?
        .get("DNSName")?
        .as_str()?
        .trim_end_matches('.');

    // Port 443 is the default HTTPS port; omit from URL
    let url = if port == 443 {
        format!("https://{dns_name}")
    } else {
        format!("https://{dns_name}:{port}")
    };

    Some(url)
}

/// Relay manager for handling cloud relay connections
pub struct RelayManager {
    config: RelayConfig,
    status: RelayStatus,
    child: Option<Child>,
}

impl RelayManager {
    /// Create a new relay manager with the given configuration
    #[must_use]
    pub fn new(config: RelayConfig) -> Self {
        let mode = match &config.mode {
            RelayMode::None => "none",
            RelayMode::TailscaleServe { .. } => "tailscale_serve",
            RelayMode::TailscaleFunnel { .. } => "tailscale_funnel",
            RelayMode::SshTunnel { .. } => "ssh_tunnel",
        };

        let status = RelayStatus {
            enabled: config.enabled,
            mode: mode.to_string(),
            url: None,
            connected: false,
        };

        Self {
            config,
            status,
            child: None,
        }
    }

    /// Start the relay (if enabled)
    ///
    /// # Errors
    ///
    /// Returns error if relay binary is missing or subprocess fails to spawn
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            tracing::debug!("relay disabled, skipping start");
            return Ok(());
        }

        let local_port = self.config.local_port;

        match self.config.mode.clone() {
            RelayMode::None => {
                tracing::debug!("relay mode is none, skipping start");
            }
            RelayMode::TailscaleServe { port } => {
                let ts = find_binary("tailscale")?;

                tracing::info!(port, local_port, "starting Tailscale Serve relay");

                let child = Command::new(&ts)
                    .args([
                        "serve",
                        "--bg",
                        &port.to_string(),
                        &format!("https+insecure://localhost:{local_port}"),
                    ])
                    .spawn()
                    .map_err(|e| {
                        Error::Config(format!("failed to spawn tailscale serve: {e}"))
                    })?;

                self.child = Some(child);

                if let Some(url) = resolve_tailscale_url(&ts, port).await {
                    tracing::info!(url, "Tailscale Serve relay available");
                    self.status.url = Some(url);
                }

                self.status.connected = true;
            }
            RelayMode::TailscaleFunnel { port, password } => {
                let ts = find_binary("tailscale")?;

                tracing::info!(
                    port,
                    local_port,
                    has_password = password.is_some(),
                    "starting Tailscale Funnel relay"
                );

                let child = Command::new(&ts)
                    .args([
                        "funnel",
                        "--bg",
                        &port.to_string(),
                        &format!("https+insecure://localhost:{local_port}"),
                    ])
                    .spawn()
                    .map_err(|e| {
                        Error::Config(format!("failed to spawn tailscale funnel: {e}"))
                    })?;

                self.child = Some(child);

                if let Some(url) = resolve_tailscale_url(&ts, port).await {
                    tracing::info!(url, "Tailscale Funnel relay available");
                    self.status.url = Some(url);
                }

                self.status.connected = true;
            }
            RelayMode::SshTunnel {
                host,
                port,
                key_path,
                user,
            } => {
                let ssh = find_binary("ssh")?;

                let ssh_user = user.as_deref().unwrap_or("root");

                tracing::info!(
                    host,
                    port,
                    local_port,
                    user = ssh_user,
                    key = key_path.as_ref().map(|p| p.display().to_string()),
                    "starting SSH tunnel relay"
                );

                let mut cmd = Command::new(&ssh);

                cmd.args([
                    "-R",
                    &format!("{port}:localhost:{local_port}"),
                ]);

                if let Some(ref key) = key_path {
                    cmd.args(["-i", &key.display().to_string()]);
                }

                cmd.args([
                    &format!("{ssh_user}@{host}"),
                    "-N",
                    "-o", "ServerAliveInterval=30",
                    "-o", "ServerAliveCountMax=3",
                    "-o", "ExitOnForwardFailure=yes",
                    "-o", "StrictHostKeyChecking=accept-new",
                ]);

                let child = cmd.spawn().map_err(|e| {
                    Error::Config(format!("failed to spawn SSH tunnel: {e}"))
                })?;

                self.child = Some(child);
                self.status.url = Some(format!("https://{host}:{port}"));
                self.status.connected = true;

                tracing::info!(
                    url = self.status.url.as_deref().unwrap_or(""),
                    "SSH tunnel relay started"
                );
            }
        }

        Ok(())
    }

    /// Stop the relay
    ///
    /// # Errors
    ///
    /// Returns error if relay cleanup fails
    pub async fn stop(&mut self) -> Result<()> {
        if !self.status.connected {
            return Ok(());
        }

        tracing::info!(mode = %self.status.mode, "stopping relay");

        // Kill child process if running
        if let Some(ref mut child) = self.child {
            if let Err(e) = child.kill().await {
                tracing::warn!(error = %e, "failed to kill relay child process");
            }
        }
        self.child = None;

        // Run Tailscale cleanup commands to remove the serve/funnel binding
        match &self.config.mode {
            RelayMode::TailscaleServe { port } => {
                if let Ok(ts) = find_binary("tailscale") {
                    let port_str = port.to_string();
                    let output = Command::new(&ts)
                        .args(["serve", "--remove", &port_str])
                        .output()
                        .await;

                    if let Err(e) = output {
                        tracing::warn!(error = %e, "failed to remove tailscale serve binding");
                    }
                }
            }
            RelayMode::TailscaleFunnel { port, .. } => {
                if let Ok(ts) = find_binary("tailscale") {
                    let port_str = port.to_string();
                    let output = Command::new(&ts)
                        .args(["funnel", "--remove", &port_str])
                        .output()
                        .await;

                    if let Err(e) = output {
                        tracing::warn!(error = %e, "failed to remove tailscale funnel binding");
                    }
                }
            }
            RelayMode::None | RelayMode::SshTunnel { .. } => {}
        }

        self.status.connected = false;
        self.status.url = None;

        Ok(())
    }

    /// Get current relay status
    #[must_use]
    pub const fn status(&self) -> &RelayStatus {
        &self.status
    }

    /// Get public URL if available
    #[must_use]
    pub fn public_url(&self) -> Option<&str> {
        self.status.url.as_deref()
    }
}
