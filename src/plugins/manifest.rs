//! Plugin manifest format (`omni.plugin.json`)

use serde::{Deserialize, Serialize};

/// Plugin manifest describing a plugin's metadata and capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g. "omni.weather")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Semver version string
    pub version: String,
    /// Short description
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin author
    #[serde(default)]
    pub author: Option<String>,
    /// What kind of plugin this is
    pub kind: PluginKind,
    /// Required permissions
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Tool definitions (for tool plugins)
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
    /// Entry point (relative path to executable or script)
    #[serde(default)]
    pub entry: Option<String>,
    /// Relative path to a skills directory within the plugin
    #[serde(default)]
    pub skills_dir: Option<String>,
    /// Transport mechanism for tool execution
    #[serde(default)]
    pub transport: PluginTransport,
    /// Environment variables to pass to the plugin process
    #[serde(default)]
    pub env: std::collections::HashMap<String, EnvValue>,
}

/// How the plugin exposes its tools
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PluginTransport {
    /// Subprocess execution (default, legacy)
    #[default]
    Subprocess,
    /// MCP server over stdio
    McpStdio,
}

/// Environment variable value (plain string or required marker)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvValue {
    /// Literal value
    Value(String),
    /// Descriptor with metadata
    Descriptor {
        /// Whether the env var is required
        #[serde(default)]
        required: bool,
        /// Default value if not required
        #[serde(default)]
        default: Option<String>,
    },
}

/// Plugin category
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Provides tool definitions for the LLM
    Tool,
    /// Provides a messaging channel adapter
    Channel,
    /// Provides an LLM provider
    Provider,
    /// Provides a skill
    Skill,
    /// Provides lifecycle hooks
    Hook,
    /// Background service
    Service,
}

/// Tool definition within a plugin manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    /// Tool name (scoped by plugin ID at runtime)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_manifest() {
        let json = r#"{
            "id": "omni.weather",
            "name": "Weather",
            "version": "1.0.0",
            "description": "Get weather forecasts",
            "author": "Omni",
            "kind": "tool",
            "permissions": ["network"],
            "tools": [
                {
                    "name": "get_forecast",
                    "description": "Get weather forecast for a location",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" }
                        },
                        "required": ["location"]
                    }
                }
            ],
            "entry": "weather.js"
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.id, "omni.weather");
        assert_eq!(manifest.kind, PluginKind::Tool);
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].name, "get_forecast");
        assert_eq!(manifest.permissions, vec!["network"]);
    }

    #[test]
    fn deserialize_minimal_manifest() {
        let json = r#"{
            "id": "omni.example",
            "name": "Example",
            "version": "0.1.0",
            "kind": "service"
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.id, "omni.example");
        assert_eq!(manifest.kind, PluginKind::Service);
        assert!(manifest.description.is_none());
        assert!(manifest.tools.is_empty());
        assert!(manifest.permissions.is_empty());
    }

    #[test]
    fn deserialize_mcp_transport() {
        let json = r#"{
            "id": "omni.github",
            "name": "GitHub",
            "version": "1.0.0",
            "kind": "tool",
            "transport": "mcp-stdio",
            "entry": "npx -y @modelcontextprotocol/server-github",
            "env": {
                "GITHUB_TOKEN": { "required": true }
            }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.transport, PluginTransport::McpStdio);
        assert!(manifest.env.contains_key("GITHUB_TOKEN"));
    }

    #[test]
    fn default_transport_is_subprocess() {
        let json = r#"{
            "id": "omni.test",
            "name": "Test",
            "version": "1.0.0",
            "kind": "tool"
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.transport, PluginTransport::Subprocess);
    }

    #[test]
    fn round_trip_all_kinds() {
        for kind_str in ["tool", "channel", "provider", "skill", "hook", "service"] {
            let json =
                format!(r#"{{"id":"test","name":"Test","version":"1.0.0","kind":"{kind_str}"}}"#);
            let manifest: PluginManifest = serde_json::from_str(&json).unwrap();
            let serialized = serde_json::to_string(&manifest).unwrap();
            assert!(serialized.contains(kind_str));
        }
    }
}
