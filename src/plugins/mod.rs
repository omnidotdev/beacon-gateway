//! Plugin system for Beacon gateway
//!
//! Plugins are discovered from `omni.plugin.json` manifests in standard
//! directories. Each plugin declares its kind (tool, channel, provider, etc.)
//! and the capabilities it provides.

pub mod discovery;
pub mod loader;
pub mod manifest;

pub use discovery::{default_plugin_dirs, discover_plugins};
pub use loader::{LoadedPlugin, PluginManager};
pub use manifest::{PluginKind, PluginManifest, PluginToolDef};
