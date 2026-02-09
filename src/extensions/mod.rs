//! Extension/plugin system for Beacon gateway
//!
//! Extensions allow dynamic loading of channels and tools. For now, this module
//! provides the trait and registry - actual dynamic loading (WASM or .so) can
//! be implemented later.
//!
//! # Example
//!
//! ```rust,ignore
//! use beacon_gateway::extensions::{Extension, ExtensionRegistry};
//!
//! let mut registry = ExtensionRegistry::new("~/.beacon/extensions".into());
//! registry.load_all()?;
//!
//! for ext in registry.list() {
//!     println!("{}: {} v{}", ext.id, ext.name, ext.version);
//! }
//! ```

use std::path::PathBuf;

use serde::Serialize;

use crate::channels::Channel;
use crate::Result;

/// Extension trait for plugins
///
/// Extensions provide a way to dynamically add channels and tools to Beacon.
/// Each extension must have a unique identifier, human-readable name, and
/// version string.
pub trait Extension: Send + Sync {
    /// Extension unique identifier
    ///
    /// Should be a lowercase, hyphenated string (e.g., "discord-voice")
    fn id(&self) -> &str;

    /// Human-readable name
    fn name(&self) -> &str;

    /// Version string (semver recommended)
    fn version(&self) -> &str;

    /// Initialize the extension
    ///
    /// Called when the extension is loaded. Use this to set up any required
    /// resources or connections.
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails
    fn init(&mut self) -> Result<()>;

    /// Get channels provided by this extension
    ///
    /// Returns a vector of channel adapters that this extension provides.
    /// These will be registered with the channel registry.
    fn channels(&self) -> Vec<Box<dyn Channel>>;

    /// Shutdown the extension
    ///
    /// Called when the extension is being unloaded. Use this to clean up
    /// resources and close connections.
    ///
    /// # Errors
    ///
    /// Returns error if shutdown fails
    fn shutdown(&mut self) -> Result<()>;
}

/// Information about a loaded extension
#[derive(Debug, Clone, Serialize)]
pub struct ExtensionInfo {
    /// Extension unique identifier
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Version string
    pub version: String,
}

/// Extension registry for managing loaded extensions
///
/// The registry handles loading, tracking, and querying extensions.
/// Extensions are loaded from a configured directory (typically ~/.beacon/extensions/).
pub struct ExtensionRegistry {
    extensions: Vec<Box<dyn Extension>>,
    extension_dir: PathBuf,
}

impl ExtensionRegistry {
    /// Create a new extension registry
    ///
    /// # Arguments
    ///
    /// * `extension_dir` - Path to the directory containing extensions
    #[must_use]
    pub fn new(extension_dir: PathBuf) -> Self {
        Self {
            extensions: Vec::new(),
            extension_dir,
        }
    }

    /// Get the extension directory path
    #[must_use]
    pub fn extension_dir(&self) -> &std::path::Path {
        &self.extension_dir
    }

    /// Load all extensions from extension directory
    ///
    /// For now, this is a no-op placeholder. Actual dynamic loading (WASM or
    /// native plugins) will be implemented later.
    ///
    /// # Errors
    ///
    /// Returns error if extension directory cannot be read or an extension
    /// fails to load
    pub fn load_all(&mut self) -> Result<()> {
        // Ensure extension directory exists
        if !self.extension_dir.exists() {
            std::fs::create_dir_all(&self.extension_dir)?;
            tracing::debug!(
                path = %self.extension_dir.display(),
                "created extension directory"
            );
        }

        // TODO: implement actual extension loading (WASM, .so, etc.)
        tracing::debug!(
            path = %self.extension_dir.display(),
            "extension loading not yet implemented"
        );

        Ok(())
    }

    /// Register an extension manually
    ///
    /// This allows programmatic registration of extensions without loading
    /// them from the filesystem.
    ///
    /// # Errors
    ///
    /// Returns error if the extension fails to initialize
    pub fn register(&mut self, mut extension: Box<dyn Extension>) -> Result<()> {
        let id = extension.id().to_string();
        let name = extension.name().to_string();

        tracing::info!(id = %id, name = %name, "registering extension");
        extension.init()?;
        self.extensions.push(extension);

        Ok(())
    }

    /// Get all channels from loaded extensions
    ///
    /// Collects and returns channels from all registered extensions.
    #[must_use]
    pub fn channels(&self) -> Vec<Box<dyn Channel>> {
        self.extensions
            .iter()
            .flat_map(|ext| ext.channels())
            .collect()
    }

    /// Get extension by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn Extension> {
        self.extensions
            .iter()
            .find(|ext| ext.id() == id)
            .map(AsRef::as_ref)
    }

    /// Get mutable extension by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Box<dyn Extension>> {
        self.extensions.iter_mut().find(|ext| ext.id() == id)
    }

    /// List all loaded extensions
    #[must_use]
    pub fn list(&self) -> Vec<ExtensionInfo> {
        self.extensions
            .iter()
            .map(|ext| ExtensionInfo {
                id: ext.id().to_string(),
                name: ext.name().to_string(),
                version: ext.version().to_string(),
            })
            .collect()
    }

    /// Shutdown all extensions
    ///
    /// Calls shutdown on each extension. Errors are logged but do not stop
    /// the shutdown process.
    pub fn shutdown_all(&mut self) {
        for ext in &mut self.extensions {
            let id = ext.id().to_string();
            if let Err(e) = ext.shutdown() {
                tracing::warn!(id = %id, error = %e, "extension shutdown failed");
            }
        }
    }

    /// Number of loaded extensions
    #[must_use]
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Check if no extensions are loaded
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        // Default to ~/.beacon/extensions/
        let extension_dir = directories::BaseDirs::new()
            .map_or_else(
                || PathBuf::from(".beacon/extensions"),
                |dirs| dirs.home_dir().join(".beacon/extensions"),
            );

        Self::new(extension_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExtension {
        id: String,
        initialized: bool,
    }

    impl MockExtension {
        fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                initialized: false,
            }
        }
    }

    impl Extension for MockExtension {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            "Mock Extension"
        }

        fn version(&self) -> &str {
            "1.0.0"
        }

        fn init(&mut self) -> Result<()> {
            self.initialized = true;
            Ok(())
        }

        fn channels(&self) -> Vec<Box<dyn Channel>> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<()> {
            self.initialized = false;
            Ok(())
        }
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ExtensionRegistry::new(PathBuf::from("/tmp/test-extensions"));
        let ext = MockExtension::new("test-ext");

        registry.register(Box::new(ext)).unwrap();

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_registry_get() {
        let mut registry = ExtensionRegistry::new(PathBuf::from("/tmp/test-extensions"));
        registry.register(Box::new(MockExtension::new("test-ext"))).unwrap();

        let ext = registry.get("test-ext");
        assert!(ext.is_some());
        assert_eq!(ext.unwrap().id(), "test-ext");

        let missing = registry.get("nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_registry_list() {
        let mut registry = ExtensionRegistry::new(PathBuf::from("/tmp/test-extensions"));
        registry.register(Box::new(MockExtension::new("ext-1"))).unwrap();
        registry.register(Box::new(MockExtension::new("ext-2"))).unwrap();

        let list = registry.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "ext-1");
        assert_eq!(list[1].id, "ext-2");
    }
}
