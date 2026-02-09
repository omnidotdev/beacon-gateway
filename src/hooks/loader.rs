//! Hook directory discovery and loading

use std::path::{Path, PathBuf};

use super::types::{HookAction, HookManifest};

/// Discovered hook from filesystem
#[derive(Debug, Clone)]
pub struct DiscoveredHook {
    /// Hook name
    pub name: String,
    /// Directory containing the hook
    pub path: PathBuf,
    /// Path to handler executable
    pub handler_path: PathBuf,
    /// Parsed manifest
    pub manifest: HookManifest,
    /// Events this hook subscribes to
    pub events: Vec<HookAction>,
}

/// Load hooks from a directory
///
/// Looks for subdirectories containing HOOK.toml and a handler
pub fn discover_hooks(hooks_dir: &Path) -> Vec<DiscoveredHook> {
    let mut hooks = Vec::new();

    if !hooks_dir.exists() {
        tracing::debug!(path = %hooks_dir.display(), "hooks directory does not exist");
        return hooks;
    }

    let entries = match std::fs::read_dir(hooks_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                path = %hooks_dir.display(),
                error = %e,
                "failed to read hooks directory"
            );
            return hooks;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        match load_hook(&path) {
            Ok(Some(hook)) => {
                tracing::info!(
                    name = %hook.name,
                    events = ?hook.events.iter().map(HookAction::as_str).collect::<Vec<_>>(),
                    "discovered hook"
                );
                hooks.push(hook);
            }
            Ok(None) => {
                // Not a hook directory, skip silently
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to load hook"
                );
            }
        }
    }

    hooks
}

/// Load a single hook from its directory
fn load_hook(dir: &Path) -> Result<Option<DiscoveredHook>, String> {
    let manifest_path = dir.join("HOOK.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }

    // Parse manifest
    let manifest_content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read HOOK.toml: {e}"))?;

    let manifest: HookManifest = toml::from_str(&manifest_content)
        .map_err(|e| format!("failed to parse HOOK.toml: {e}"))?;

    // Check if enabled
    if !manifest.enabled {
        tracing::debug!(name = %manifest.name, "hook disabled, skipping");
        return Ok(None);
    }

    // Check requirements
    if !manifest.requires.is_satisfied() {
        tracing::debug!(
            name = %manifest.name,
            "hook requirements not satisfied, skipping"
        );
        return Ok(None);
    }

    // Find handler
    let handler_path = find_handler(dir)?;

    // Parse events
    let events: Vec<_> = manifest
        .events
        .iter()
        .filter_map(|e| {
            let action = HookAction::from_str(e);
            if action.is_none() {
                tracing::warn!(
                    hook = %manifest.name,
                    event = %e,
                    "unknown event type, ignoring"
                );
            }
            action
        })
        .collect();

    if events.is_empty() {
        tracing::warn!(hook = %manifest.name, "no valid events, skipping");
        return Ok(None);
    }

    Ok(Some(DiscoveredHook {
        name: manifest.name.clone(),
        path: dir.to_path_buf(),
        handler_path,
        manifest,
        events,
    }))
}

/// Find the handler executable in a hook directory
fn find_handler(dir: &Path) -> Result<PathBuf, String> {
    // Check for common handler names
    let candidates = [
        "handler",
        "handler.sh",
        "handler.py",
        "handler.js",
        "handler.ts",
        "handler.rb",
    ];

    for name in &candidates {
        let path = dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }

    Err("no handler found (expected handler, handler.sh, handler.py, etc.)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_discover_empty_dir() {
        let dir = TempDir::new().unwrap();
        let hooks = discover_hooks(dir.path());
        assert!(hooks.is_empty());
    }

    #[test]
    fn test_discover_valid_hook() {
        let dir = TempDir::new().unwrap();
        let hook_dir = dir.path().join("my-hook");
        fs::create_dir(&hook_dir).unwrap();

        // Create manifest
        fs::write(
            hook_dir.join("HOOK.toml"),
            r#"
name = "my-hook"
description = "Test hook"
events = ["message:received"]
"#,
        )
        .unwrap();

        // Create handler
        fs::write(hook_dir.join("handler.sh"), "#!/bin/bash\necho '{}'").unwrap();

        let hooks = discover_hooks(dir.path());
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "my-hook");
        assert_eq!(hooks[0].events, vec![HookAction::MessageReceived]);
    }

    #[test]
    fn test_skip_disabled_hook() {
        let dir = TempDir::new().unwrap();
        let hook_dir = dir.path().join("disabled-hook");
        fs::create_dir(&hook_dir).unwrap();

        fs::write(
            hook_dir.join("HOOK.toml"),
            r#"
name = "disabled-hook"
events = ["message:received"]
enabled = false
"#,
        )
        .unwrap();

        fs::write(hook_dir.join("handler.sh"), "#!/bin/bash").unwrap();

        let hooks = discover_hooks(dir.path());
        assert!(hooks.is_empty());
    }
}
