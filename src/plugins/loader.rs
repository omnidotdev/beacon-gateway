//! Plugin loader and lifecycle manager

use std::collections::HashMap;
use std::path::PathBuf;

use super::discovery::discover_plugins;
use super::manifest::{PluginManifest, PluginToolDef};

/// A discovered and loaded plugin
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Plugin manifest
    pub manifest: PluginManifest,
    /// Directory containing the plugin
    pub path: PathBuf,
    /// Whether the plugin is currently enabled
    pub enabled: bool,
}

/// Manage discovered plugins
#[derive(Debug)]
pub struct PluginManager {
    plugins: HashMap<String, LoadedPlugin>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    /// Create a new empty plugin manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Discover and load plugins from the given directories
    ///
    /// Returns the IDs of all newly loaded plugins
    pub fn load_all(&mut self, dirs: &[PathBuf]) -> Vec<String> {
        let discovered = discover_plugins(dirs);
        let mut loaded_ids = Vec::new();

        for (path, manifest) in discovered {
            let id = manifest.id.clone();

            if self.plugins.contains_key(&id) {
                tracing::debug!(plugin_id = %id, "plugin already loaded, skipping");
                continue;
            }

            tracing::info!(
                plugin_id = %id,
                name = %manifest.name,
                version = %manifest.version,
                kind = ?manifest.kind,
                "loaded plugin"
            );

            self.plugins.insert(
                id.clone(),
                LoadedPlugin {
                    manifest,
                    path,
                    enabled: true,
                },
            );

            loaded_ids.push(id);
        }

        loaded_ids
    }

    /// Get a plugin by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(id)
    }

    /// List all loaded plugins
    #[must_use]
    pub fn list(&self) -> Vec<&LoadedPlugin> {
        self.plugins.values().collect()
    }

    /// Enable a plugin, returning true if found
    pub fn enable(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.enabled = true;
            tracing::info!(plugin_id = %id, "plugin enabled");
            true
        } else {
            false
        }
    }

    /// Disable a plugin, returning true if found
    pub fn disable(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.enabled = false;
            tracing::info!(plugin_id = %id, "plugin disabled");
            true
        } else {
            false
        }
    }

    /// Collect all tool definitions from enabled tool plugins
    ///
    /// Tool names are scoped as `plugin_id::tool_name`
    #[must_use]
    pub fn tools(&self) -> Vec<(String, PluginToolDef)> {
        self.plugins
            .values()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.manifest.tools.iter().map(move |tool| {
                    let scoped_name = format!("{}::{}", p.manifest.id, tool.name);
                    (scoped_name, tool.clone())
                })
            })
            .collect()
    }

    /// Number of loaded plugins
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether no plugins are loaded
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_directory() {
        let dir = tempfile::tempdir().unwrap();

        // Create a plugin
        let plugin_dir = dir.path().join("test-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("omni.plugin.json"),
            r#"{
                "id": "omni.test",
                "name": "Test",
                "version": "1.0.0",
                "kind": "tool",
                "tools": [{
                    "name": "greet",
                    "description": "Say hello",
                    "input_schema": {"type": "object"}
                }]
            }"#,
        )
        .unwrap();

        let mut manager = PluginManager::new();
        let loaded = manager.load_all(&[dir.path().to_path_buf()]);

        assert_eq!(loaded, vec!["omni.test"]);
        assert_eq!(manager.len(), 1);

        let plugin = manager.get("omni.test").unwrap();
        assert!(plugin.enabled);
        assert_eq!(plugin.manifest.name, "Test");
    }

    #[test]
    fn enable_disable() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("omni.plugin.json"),
            r#"{"id":"p1","name":"P","version":"1.0.0","kind":"tool"}"#,
        )
        .unwrap();

        let mut manager = PluginManager::new();
        manager.load_all(&[dir.path().to_path_buf()]);

        assert!(manager.get("p1").unwrap().enabled);
        assert!(manager.disable("p1"));
        assert!(!manager.get("p1").unwrap().enabled);
        assert!(manager.enable("p1"));
        assert!(manager.get("p1").unwrap().enabled);
    }

    #[test]
    fn disable_nonexistent() {
        let mut manager = PluginManager::new();
        assert!(!manager.disable("nonexistent"));
        assert!(!manager.enable("nonexistent"));
    }

    #[test]
    fn tools_from_enabled_plugins() {
        let dir = tempfile::tempdir().unwrap();

        // Plugin with tools
        let p1_dir = dir.path().join("p1");
        std::fs::create_dir(&p1_dir).unwrap();
        std::fs::write(
            p1_dir.join("omni.plugin.json"),
            r#"{
                "id": "omni.p1",
                "name": "P1",
                "version": "1.0.0",
                "kind": "tool",
                "tools": [
                    {"name": "a", "description": "Tool A", "input_schema": {}},
                    {"name": "b", "description": "Tool B", "input_schema": {}}
                ]
            }"#,
        )
        .unwrap();

        // Plugin without tools
        let p2_dir = dir.path().join("p2");
        std::fs::create_dir(&p2_dir).unwrap();
        std::fs::write(
            p2_dir.join("omni.plugin.json"),
            r#"{"id":"omni.p2","name":"P2","version":"1.0.0","kind":"service"}"#,
        )
        .unwrap();

        let mut manager = PluginManager::new();
        manager.load_all(&[dir.path().to_path_buf()]);

        let tools = manager.tools();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"omni.p1::a"));
        assert!(names.contains(&"omni.p1::b"));

        // Disable p1, tools should be empty
        manager.disable("omni.p1");
        assert!(manager.tools().is_empty());
    }

    #[test]
    fn no_duplicate_loads() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("omni.plugin.json"),
            r#"{"id":"p1","name":"P","version":"1.0.0","kind":"tool"}"#,
        )
        .unwrap();

        let mut manager = PluginManager::new();
        let first = manager.load_all(&[dir.path().to_path_buf()]);
        let second = manager.load_all(&[dir.path().to_path_buf()]);

        assert_eq!(first.len(), 1);
        assert!(second.is_empty()); // Already loaded
        assert_eq!(manager.len(), 1);
    }
}
