//! Install automation for skill dependencies

use std::path::PathBuf;

use crate::Error;

use super::types::{
    InstallKind, NodeManager, SkillInstallPreferences, SkillInstallResult, SkillInstallSpec,
    has_binary,
};

/// Select the best install spec for the current platform and preferences
#[must_use]
pub fn select_install_spec<'a>(
    specs: &'a [SkillInstallSpec],
    prefs: &SkillInstallPreferences,
) -> Option<&'a SkillInstallSpec> {
    let current_os = std::env::consts::OS;

    // Filter by OS first
    let compatible: Vec<&SkillInstallSpec> = specs
        .iter()
        .filter(|s| s.os.is_empty() || s.os.iter().any(|o| o == current_os))
        .collect();

    if compatible.is_empty() {
        return None;
    }

    // Preference chain
    if prefs.prefer_brew && has_binary("brew") {
        if let Some(spec) = compatible.iter().find(|s| s.kind == InstallKind::Brew) {
            return Some(spec);
        }
    }

    // Uv
    if has_binary("uv") {
        if let Some(spec) = compatible.iter().find(|s| s.kind == InstallKind::Uv) {
            return Some(spec);
        }
    }

    // Node (check if preferred manager is on PATH)
    let node_bin = match prefs.node_manager {
        NodeManager::Npm => "npm",
        NodeManager::Pnpm => "pnpm",
        NodeManager::Yarn => "yarn",
        NodeManager::Bun => "bun",
    };
    if has_binary(node_bin) {
        if let Some(spec) = compatible.iter().find(|s| s.kind == InstallKind::Node) {
            return Some(spec);
        }
    }

    // Go
    if has_binary("go") {
        if let Some(spec) = compatible.iter().find(|s| s.kind == InstallKind::Go) {
            return Some(spec);
        }
    }

    // Download (always available)
    if let Some(spec) = compatible.iter().find(|s| s.kind == InstallKind::Download) {
        return Some(spec);
    }

    // Fall back to first compatible
    compatible.into_iter().next()
}

/// Execute an install spec and verify binaries are available
///
/// # Errors
///
/// Returns error if the install command fails to spawn
pub async fn execute_install(
    spec: &SkillInstallSpec,
    prefs: &SkillInstallPreferences,
) -> Result<SkillInstallResult, Error> {
    let result = match spec.kind {
        InstallKind::Brew => install_brew(spec).await,
        InstallKind::Node => install_node(spec, prefs).await,
        InstallKind::Go => install_go(spec).await,
        InstallKind::Uv => install_uv(spec).await,
        InstallKind::Download => install_download(spec).await,
    };

    let mut result = result?;

    // Post-install verification
    if result.ok && !spec.bins.is_empty() {
        let missing: Vec<&str> = spec
            .bins
            .iter()
            .filter(|b| !has_binary(b))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            result.warnings.push(format!(
                "install succeeded but binaries not found on PATH: {}",
                missing.join(", ")
            ));
        }
    }

    Ok(result)
}

/// Run a command and capture output
async fn run_command(program: &str, args: &[&str]) -> Result<SkillInstallResult, Error> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| Error::Install(format!("failed to run {program}: {e}")))?;

    let code = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let ok = output.status.success();

    Ok(SkillInstallResult {
        ok,
        message: if ok {
            format!("{program} completed successfully")
        } else {
            format!("{program} exited with code {}", code.unwrap_or(-1))
        },
        stdout,
        stderr,
        code,
        warnings: vec![],
    })
}

async fn install_brew(spec: &SkillInstallSpec) -> Result<SkillInstallResult, Error> {
    let formula = spec
        .formula
        .as_deref()
        .ok_or_else(|| Error::Install("brew install requires `formula` field".to_string()))?;

    run_command("brew", &["install", formula]).await
}

async fn install_node(
    spec: &SkillInstallSpec,
    prefs: &SkillInstallPreferences,
) -> Result<SkillInstallResult, Error> {
    let package = spec
        .package
        .as_deref()
        .ok_or_else(|| Error::Install("node install requires `package` field".to_string()))?;

    let manager = match prefs.node_manager {
        NodeManager::Npm => "npm",
        NodeManager::Pnpm => "pnpm",
        NodeManager::Yarn => "yarn",
        NodeManager::Bun => "bun",
    };

    run_command(manager, &["install", "-g", "--ignore-scripts", package]).await
}

async fn install_go(spec: &SkillInstallSpec) -> Result<SkillInstallResult, Error> {
    let module = spec
        .module
        .as_deref()
        .ok_or_else(|| Error::Install("go install requires `module` field".to_string()))?;

    run_command("go", &["install", module]).await
}

async fn install_uv(spec: &SkillInstallSpec) -> Result<SkillInstallResult, Error> {
    let package = spec
        .package
        .as_deref()
        .ok_or_else(|| Error::Install("uv install requires `package` field".to_string()))?;

    run_command("uv", &["tool", "install", package]).await
}

async fn install_download(spec: &SkillInstallSpec) -> Result<SkillInstallResult, Error> {
    let url = spec
        .url
        .as_deref()
        .ok_or_else(|| Error::Install("download install requires `url` field".to_string()))?;

    let target_dir = spec.target_dir.as_deref().map_or_else(
        || {
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".local").join("bin"))
                .unwrap_or_else(|| PathBuf::from("/usr/local/bin"))
        },
        PathBuf::from,
    );

    // Ensure target dir exists
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| Error::Install(format!("failed to create target dir: {e}")))?;

    // Download to temp file
    let response = reqwest::get(url)
        .await
        .map_err(|e| Error::Install(format!("download failed: {e}")))?;

    if !response.status().is_success() {
        return Ok(SkillInstallResult {
            ok: false,
            message: format!("download returned HTTP {}", response.status()),
            stdout: String::new(),
            stderr: String::new(),
            code: None,
            warnings: vec![],
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Install(format!("failed to read response body: {e}")))?;

    let temp_dir = tempfile::tempdir()
        .map_err(|e| Error::Install(format!("failed to create temp dir: {e}")))?;
    let temp_path = temp_dir.path().join("download");
    tokio::fs::write(&temp_path, &bytes)
        .await
        .map_err(|e: std::io::Error| Error::Install(format!("failed to write temp file: {e}")))?;

    // Detect archive type and extract
    let archive_type = spec
        .archive
        .as_deref()
        .or_else(|| {
            if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
                Some("tar.gz")
            } else if url.ends_with(".tar.bz2") {
                Some("tar.bz2")
            } else if url.ends_with(".zip") {
                Some("zip")
            } else {
                None
            }
        });

    let strip = spec.strip_components.unwrap_or(0);
    let target_str = target_dir.to_string_lossy().to_string();

    match archive_type {
        Some("tar.gz" | "tgz") => {
            let strip_arg = format!("--strip-components={strip}");
            run_command(
                "tar",
                &["xzf", &temp_path.to_string_lossy(), &strip_arg, "-C", &target_str],
            )
            .await
        }
        Some("tar.bz2") => {
            let strip_arg = format!("--strip-components={strip}");
            run_command(
                "tar",
                &["xjf", &temp_path.to_string_lossy(), &strip_arg, "-C", &target_str],
            )
            .await
        }
        Some("zip") => {
            run_command(
                "unzip",
                &["-o", &temp_path.to_string_lossy(), "-d", &target_str],
            )
            .await
        }
        _ => {
            // Not an archive — move directly to target
            let file_name = url
                .rsplit('/')
                .next()
                .unwrap_or("downloaded_binary");
            let dest = target_dir.join(file_name);
            tokio::fs::copy(&temp_path, &dest)
                .await
                .map_err(|e: std::io::Error| Error::Install(format!("failed to copy binary: {e}")))?;

            // Make executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&dest, perms)
                    .map_err(|e| Error::Install(format!("failed to chmod: {e}")))?;
            }

            Ok(SkillInstallResult {
                ok: true,
                message: format!("downloaded to {}", dest.display()),
                stdout: String::new(),
                stderr: String::new(),
                code: Some(0),
                warnings: vec![],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brew_spec() -> SkillInstallSpec {
        SkillInstallSpec {
            kind: InstallKind::Brew,
            label: None,
            bins: vec!["jq".to_string()],
            os: vec![],
            formula: Some("jq".to_string()),
            package: None,
            module: None,
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        }
    }

    fn node_spec() -> SkillInstallSpec {
        SkillInstallSpec {
            kind: InstallKind::Node,
            label: None,
            bins: vec!["prettier".to_string()],
            os: vec![],
            formula: None,
            package: Some("prettier".to_string()),
            module: None,
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        }
    }

    fn go_spec() -> SkillInstallSpec {
        SkillInstallSpec {
            kind: InstallKind::Go,
            label: None,
            bins: vec![],
            os: vec![],
            formula: None,
            package: None,
            module: Some("github.com/example/tool@latest".to_string()),
            url: None,
            archive: None,
            strip_components: None,
            target_dir: None,
        }
    }

    fn download_spec() -> SkillInstallSpec {
        SkillInstallSpec {
            kind: InstallKind::Download,
            label: None,
            bins: vec![],
            os: vec![],
            formula: None,
            package: None,
            module: None,
            url: Some("https://example.com/tool.tar.gz".to_string()),
            archive: Some("tar.gz".to_string()),
            strip_components: Some(1),
            target_dir: None,
        }
    }

    #[test]
    fn select_install_spec_prefers_brew() {
        let specs = vec![node_spec(), brew_spec(), go_spec()];
        let prefs = SkillInstallPreferences {
            prefer_brew: true,
            node_manager: NodeManager::Npm,
        };

        if has_binary("brew") {
            let selected = select_install_spec(&specs, &prefs);
            assert_eq!(selected.unwrap().kind, InstallKind::Brew);
        }
    }

    #[test]
    fn select_install_spec_fallback_order() {
        let specs = vec![go_spec(), download_spec()];
        let prefs = SkillInstallPreferences {
            prefer_brew: false,
            node_manager: NodeManager::Npm,
        };

        let selected = select_install_spec(&specs, &prefs);
        assert!(selected.is_some());
        // If go isn't installed, should fall to download
        if !has_binary("go") {
            assert_eq!(selected.unwrap().kind, InstallKind::Download);
        }
    }

    #[test]
    fn select_install_spec_os_filter() {
        let mut spec = brew_spec();
        spec.os = vec!["nonexistent_os".to_string()];

        let prefs = SkillInstallPreferences::default();
        let specs = [spec];
        let selected = select_install_spec(&specs, &prefs);
        assert!(selected.is_none());
    }

    #[test]
    fn select_install_spec_empty() {
        let prefs = SkillInstallPreferences::default();
        let selected = select_install_spec(&[], &prefs);
        assert!(selected.is_none());
    }
}
