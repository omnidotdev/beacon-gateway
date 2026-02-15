//! Plugin discovery - scan directories for `omni.plugin.json` manifests

use std::path::{Path, PathBuf};

use super::manifest::PluginManifest;

/// Scan plugin directories for manifests
///
/// Looks for `omni.plugin.json` files in immediate subdirectories of each
/// search path. Returns `(directory, manifest)` pairs for each valid manifest.
#[must_use]
pub fn discover_plugins(dirs: &[PathBuf]) -> Vec<(PathBuf, PluginManifest)> {
    let mut results = Vec::new();

    for dir in dirs {
        if !dir.is_dir() {
            tracing::debug!(path = %dir.display(), "plugin directory does not exist, skipping");
            continue;
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            tracing::warn!(path = %dir.display(), "failed to read plugin directory");
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("omni.plugin.json");
            if let Some(manifest) = load_manifest(&manifest_path) {
                tracing::debug!(
                    plugin_id = %manifest.id,
                    path = %path.display(),
                    "discovered plugin"
                );
                results.push((path, manifest));
            }
        }
    }

    results
}

/// Load and parse a single manifest file
fn load_manifest(path: &Path) -> Option<PluginManifest> {
    let content = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<PluginManifest>(&content) {
        Ok(manifest) => Some(manifest),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to parse plugin manifest"
            );
            None
        }
    }
}

/// Default plugin search directories
#[must_use]
pub fn default_plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(config_dir) = directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf()) {
        dirs.push(config_dir.join("omni").join("plugins"));
    }

    if let Some(data_dir) = directories::BaseDirs::new().map(|d| d.data_dir().to_path_buf()) {
        dirs.push(data_dir.join("omni").join("plugins"));
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let results = discover_plugins(&[dir.path().to_path_buf()]);
        assert!(results.is_empty());
    }

    #[test]
    fn discover_valid_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        let manifest = r#"{
            "id": "omni.test",
            "name": "Test Plugin",
            "version": "1.0.0",
            "kind": "tool"
        }"#;
        std::fs::write(plugin_dir.join("omni.plugin.json"), manifest).unwrap();

        let results = discover_plugins(&[dir.path().to_path_buf()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.id, "omni.test");
        assert_eq!(results[0].0, plugin_dir);
    }

    #[test]
    fn skip_invalid_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("bad-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        std::fs::write(plugin_dir.join("omni.plugin.json"), "not valid json").unwrap();

        let results = discover_plugins(&[dir.path().to_path_buf()]);
        assert!(results.is_empty());
    }

    #[test]
    fn skip_nonexistent_dir() {
        let results = discover_plugins(&[PathBuf::from("/nonexistent/path")]);
        assert!(results.is_empty());
    }

    #[test]
    fn default_dirs_not_empty() {
        let dirs = default_plugin_dirs();
        // Should have at least one directory configured
        assert!(!dirs.is_empty());
    }
}
