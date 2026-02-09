//! Tool policy for channel-based tool restrictions

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Tool profile defining a set of allowed tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolProfile {
    /// No tools - pure conversation
    None,
    /// Minimal tools
    Minimal,
    /// Read-only tools + web search (for chat channels)
    #[default]
    Messaging,
    /// All tools available (for trusted local users)
    Full,
    /// Custom profile (requires explicit tool list)
    Custom,
}

impl ToolProfile {
    /// Get the list of allowed tool names for this profile
    #[must_use]
    pub const fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Full => &[
                "shell",
                "read_file",
                "write_file",
                "web_search",
                "code_search",
                "memory_search",
                "memory_store",
                "todo_read",
                "todo_write",
            ],
            Self::Messaging => &["web_search", "read_file"],
            Self::Minimal => &["web_search"],
            Self::None | Self::Custom => &[],
        }
    }
}

/// Tool policy configuration from persona
///
/// Maps channel names to tool profiles. Special keys:
/// - "default": fallback for unspecified channels
/// - "voice": voice/audio channel
/// - "discord", "telegram", "slack": messaging platforms
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ToolPolicyConfig {
    /// Channel to profile mapping
    pub channels: HashMap<String, ToolProfile>,
}

/// Tool policy for restricting tools based on channel
#[derive(Debug, Clone)]
pub struct ToolPolicy {
    /// Profile per channel
    channel_profiles: HashMap<String, ToolProfile>,
    /// Default profile
    default_profile: ToolProfile,
}

impl ToolPolicy {
    /// Create a new tool policy from configuration
    #[must_use]
    pub fn new(config: &ToolPolicyConfig) -> Self {
        let mut channel_profiles = config.channels.clone();
        let default_profile = channel_profiles
            .remove("default")
            .unwrap_or(ToolProfile::Messaging);

        Self {
            channel_profiles,
            default_profile,
        }
    }

    /// Create default policy (voice=full, messaging channels=messaging)
    #[must_use]
    pub fn default_policy() -> Self {
        let mut channels = HashMap::new();
        channels.insert("default".to_string(), ToolProfile::Messaging);
        channels.insert("voice".to_string(), ToolProfile::Full);
        channels.insert("discord".to_string(), ToolProfile::Messaging);
        channels.insert("telegram".to_string(), ToolProfile::Messaging);
        channels.insert("slack".to_string(), ToolProfile::Messaging);

        Self::new(&ToolPolicyConfig { channels })
    }

    /// Get the profile for a channel
    #[must_use]
    pub fn profile_for(&self, channel: &str) -> ToolProfile {
        self.channel_profiles
            .get(channel)
            .copied()
            .unwrap_or(self.default_profile)
    }

    /// Get allowed tool names for a channel
    #[must_use]
    pub fn allowed_tools(&self, channel: &str) -> Vec<String> {
        self.profile_for(channel)
            .allowed_tools()
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    /// Check if a specific tool is allowed for a channel
    #[must_use]
    pub fn is_allowed(&self, channel: &str, tool: &str) -> bool {
        let allowed: HashSet<&str> = self
            .profile_for(channel)
            .allowed_tools()
            .iter()
            .copied()
            .collect();
        allowed.contains(tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = ToolPolicy::default_policy();

        // Voice gets full access
        assert!(policy.is_allowed("voice", "shell"));
        assert!(policy.is_allowed("voice", "write_file"));

        // Messaging channels are restricted
        assert!(policy.is_allowed("telegram", "web_search"));
        assert!(policy.is_allowed("telegram", "read_file"));
        assert!(!policy.is_allowed("telegram", "shell"));
        assert!(!policy.is_allowed("telegram", "write_file"));

        // Unknown channels use default
        assert!(!policy.is_allowed("unknown", "shell"));
    }

    #[test]
    fn test_profile_tools() {
        assert!(!ToolProfile::Full.allowed_tools().is_empty());
        assert!(!ToolProfile::Messaging.allowed_tools().is_empty());
        assert!(ToolProfile::None.allowed_tools().is_empty());
    }

    #[test]
    fn test_json_deserialization() {
        let json = r#"{
            "default": "messaging",
            "voice": "full",
            "discord": "messaging"
        }"#;

        let config: ToolPolicyConfig = serde_json::from_str(json).unwrap();
        let policy = ToolPolicy::new(&config);

        assert!(policy.is_allowed("voice", "shell"));
        assert!(!policy.is_allowed("discord", "shell"));
    }
}
