//! Cloud relay configuration for optional remote access
//!
//! Supports multiple relay modes:
//! - Tailscale Serve: tailnet-only access using Tailscale identity
//! - Tailscale Funnel: public HTTPS access
//! - SSH tunnel: reverse tunnel to a remote host

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Cloud relay configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Enable cloud relay
    pub enabled: bool,
    /// Relay mode
    pub mode: RelayMode,
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
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = std::env::var("BEACON_RELAY_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if !enabled {
            return Self::default();
        }

        let mode = std::env::var("BEACON_RELAY_MODE")
            .ok()
            .map(|m| Self::parse_mode(&m))
            .unwrap_or_default();

        Self { enabled, mode }
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

/// Relay manager for handling cloud relay connections
pub struct RelayManager {
    config: RelayConfig,
    status: RelayStatus,
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

        Self { config, status }
    }

    /// Start the relay (if enabled)
    ///
    /// # Errors
    ///
    /// Returns error if relay fails to start
    pub fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            tracing::debug!("relay disabled, skipping start");
            return Ok(());
        }

        match &self.config.mode {
            RelayMode::None => {
                tracing::debug!("relay mode is none, skipping start");
            }
            RelayMode::TailscaleServe { port } => {
                tracing::info!(port, "starting Tailscale Serve relay");
                // TODO: implement Tailscale Serve integration
                // `tailscale serve --bg https+insecure://localhost:{port}`
                self.status.connected = true;
            }
            RelayMode::TailscaleFunnel { port, password } => {
                tracing::info!(
                    port,
                    has_password = password.is_some(),
                    "starting Tailscale Funnel relay"
                );
                // TODO: implement Tailscale Funnel integration
                // `tailscale funnel --bg https+insecure://localhost:{port}`
                self.status.connected = true;
            }
            RelayMode::SshTunnel {
                host,
                port,
                key_path,
                user,
            } => {
                tracing::info!(
                    host,
                    port,
                    user = user.as_deref().unwrap_or("(default)"),
                    key = key_path.as_ref().map(|p| p.display().to_string()),
                    "starting SSH tunnel relay"
                );
                // TODO: implement SSH tunnel
                // `ssh -R {port}:localhost:{local_port} {user}@{host}`
                self.status.connected = true;
            }
        }

        Ok(())
    }

    /// Stop the relay
    ///
    /// # Errors
    ///
    /// Returns error if relay fails to stop
    pub fn stop(&mut self) -> Result<()> {
        if !self.status.connected {
            return Ok(());
        }

        tracing::info!(mode = %self.status.mode, "stopping relay");

        // TODO: implement actual cleanup for each mode
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
