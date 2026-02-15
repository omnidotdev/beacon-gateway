//! Platform-specific service lifecycle management
//!
//! Install, uninstall, and query the Beacon gateway as a system service

use std::path::PathBuf;

use crate::{Error, Result};

/// Service status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceStatus {
    /// Service is running
    Running,
    /// Service is installed but not running
    Stopped,
    /// Service is not installed
    NotInstalled,
    /// Status could not be determined
    Unknown(String),
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::NotInstalled => write!(f, "not installed"),
            Self::Unknown(msg) => write!(f, "unknown ({msg})"),
        }
    }
}

/// Service configuration
pub struct ServiceConfig {
    /// Path to the beacon binary
    pub binary_path: PathBuf,
    /// Persona to use
    pub persona: String,
    /// Port to listen on
    pub port: u16,
    /// Extra arguments
    pub extra_args: Vec<String>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("beacon"),
            persona: "orin".to_string(),
            port: 18789,
            extra_args: Vec::new(),
        }
    }
}

/// Install beacon as a system service
///
/// # Errors
///
/// Returns error if service installation fails
pub fn install_service(config: &ServiceConfig) -> Result<()> {
    #[cfg(target_os = "macos")]
    return install_launchd(config);

    #[cfg(target_os = "linux")]
    return install_systemd(config);

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = config;
        Err(Error::Config(
            "service installation not supported on this platform".to_string(),
        ))
    }
}

/// Uninstall the beacon system service
///
/// # Errors
///
/// Returns error if service removal fails
pub fn uninstall_service() -> Result<()> {
    #[cfg(target_os = "macos")]
    return uninstall_launchd();

    #[cfg(target_os = "linux")]
    return uninstall_systemd();

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(Error::Config(
        "service management not supported on this platform".to_string(),
    ))
}

/// Query beacon service status
///
/// # Errors
///
/// Returns error if status cannot be determined
pub fn service_status() -> Result<ServiceStatus> {
    #[cfg(target_os = "macos")]
    return launchd_status();

    #[cfg(target_os = "linux")]
    return systemd_status();

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Ok(ServiceStatus::Unknown(
        "platform not supported".to_string(),
    ))
}

/// Restart the beacon service
///
/// # Errors
///
/// Returns error if restart fails
pub fn restart_service() -> Result<()> {
    #[cfg(target_os = "macos")]
    return restart_launchd();

    #[cfg(target_os = "linux")]
    return restart_systemd();

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(Error::Config(
        "service management not supported on this platform".to_string(),
    ))
}

/// Get the service log file path
#[must_use]
pub fn log_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| {
        dirs.home_dir()
            .join(".beacon")
            .join("logs")
            .join("beacon.log")
    })
}

// --- macOS (launchd) ---

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "dev.omni.beacon";

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn install_launchd(config: &ServiceConfig) -> Result<()> {
    let log_dir = directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".beacon/logs"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    std::fs::create_dir_all(&log_dir)?;

    let binary = config.binary_path.display();
    let stdout_log = log_dir.join("beacon.log").display().to_string();
    let stderr_log = log_dir.join("beacon.err.log").display().to_string();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--persona</string>
        <string>{persona}</string>
        <string>--port</string>
        <string>{port}</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout_log}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>
</dict>
</plist>"#,
        persona = config.persona,
        port = config.port,
    );

    let path = plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, plist)?;

    // Load the agent
    let output = std::process::Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&path)
        .output()
        .map_err(|e| Error::Config(format!("failed to run launchctl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Config(format!("launchctl load failed: {stderr}")));
    }

    tracing::info!(path = %path.display(), "installed LaunchAgent");
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<()> {
    let path = plist_path();

    if path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload"])
            .arg(&path)
            .output();

        std::fs::remove_file(&path)?;
        tracing::info!("uninstalled LaunchAgent");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn launchd_status() -> Result<ServiceStatus> {
    if !plist_path().exists() {
        return Ok(ServiceStatus::NotInstalled);
    }

    let output = std::process::Command::new("launchctl")
        .args(["list", LAUNCHD_LABEL])
        .output()
        .map_err(|e| Error::Config(format!("failed to run launchctl: {e}")))?;

    if output.status.success() {
        Ok(ServiceStatus::Running)
    } else {
        Ok(ServiceStatus::Stopped)
    }
}

#[cfg(target_os = "macos")]
fn restart_launchd() -> Result<()> {
    let _ = std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{}/{LAUNCHD_LABEL}", unsafe { libc::getuid() })])
        .output();

    Ok(())
}

// --- Linux (systemd) ---

#[cfg(target_os = "linux")]
const SYSTEMD_SERVICE: &str = "beacon";

#[cfg(target_os = "linux")]
fn service_file_path() -> PathBuf {
    let config_dir = directories::BaseDirs::new()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });

    config_dir
        .join("systemd/user")
        .join(format!("{SYSTEMD_SERVICE}.service"))
}

#[cfg(target_os = "linux")]
fn install_systemd(config: &ServiceConfig) -> Result<()> {
    let log_dir = directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".beacon/logs"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    std::fs::create_dir_all(&log_dir)?;

    let binary = config.binary_path.display();

    let unit = format!(
        r#"[Unit]
Description=Beacon Gateway
After=network.target

[Service]
Type=simple
ExecStart={binary} --persona {persona} --port {port} --foreground
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
"#,
        persona = config.persona,
        port = config.port,
    );

    let path = service_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, unit)?;

    // Reload and enable
    run_systemctl(&["--user", "daemon-reload"])?;
    run_systemctl(&["--user", "enable", "--now", SYSTEMD_SERVICE])?;

    tracing::info!(path = %path.display(), "installed systemd user service");
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<()> {
    let _ = run_systemctl(&["--user", "disable", "--now", SYSTEMD_SERVICE]);

    let path = service_file_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
        let _ = run_systemctl(&["--user", "daemon-reload"]);
        tracing::info!("uninstalled systemd user service");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn systemd_status() -> Result<ServiceStatus> {
    if !service_file_path().exists() {
        return Ok(ServiceStatus::NotInstalled);
    }

    let output = std::process::Command::new("systemctl")
        .args(["--user", "is-active", SYSTEMD_SERVICE])
        .output()
        .map_err(|e| Error::Config(format!("failed to run systemctl: {e}")))?;

    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match status.as_str() {
        "active" => Ok(ServiceStatus::Running),
        "inactive" | "failed" => Ok(ServiceStatus::Stopped),
        other => Ok(ServiceStatus::Unknown(other.to_string())),
    }
}

#[cfg(target_os = "linux")]
fn restart_systemd() -> Result<()> {
    run_systemctl(&["--user", "restart", SYSTEMD_SERVICE])
}

#[cfg(target_os = "linux")]
fn run_systemctl(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("failed to run systemctl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Config(format!(
            "systemctl {} failed: {stderr}",
            args.join(" ")
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_status_display() {
        assert_eq!(ServiceStatus::Running.to_string(), "running");
        assert_eq!(ServiceStatus::Stopped.to_string(), "stopped");
        assert_eq!(ServiceStatus::NotInstalled.to_string(), "not installed");
    }

    #[test]
    fn default_service_config() {
        let config = ServiceConfig::default();
        assert_eq!(config.persona, "orin");
        assert_eq!(config.port, 18789);
    }

    #[test]
    fn log_path_exists() {
        let path = log_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("beacon.log"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_service_path() {
        let path = service_file_path();
        assert!(path.to_string_lossy().contains("beacon.service"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_path() {
        let path = plist_path();
        assert!(path.to_string_lossy().contains("dev.omni.beacon.plist"));
    }
}
