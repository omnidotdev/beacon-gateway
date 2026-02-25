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
    /// Parse a tool profile from a string value
    #[must_use]
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "messaging" => Some(Self::Messaging),
            "full" => Some(Self::Full),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

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

    /// Apply environment variable overrides to this policy.
    ///
    /// Reads:
    /// - `BEACON_TOOL_PROFILE` for the default profile
    /// - `BEACON_TOOL_PROFILE_TELEGRAM`, `BEACON_TOOL_PROFILE_DISCORD`, etc. for per-channel
    ///
    /// Environment overrides take precedence over persona JSON configuration.
    #[must_use]
    pub fn with_env_overrides(mut self) -> Self {
        // Global default override
        if let Ok(val) = std::env::var("BEACON_TOOL_PROFILE") {
            if let Some(profile) = ToolProfile::from_str_value(&val) {
                self.default_profile = profile;
                tracing::info!(profile = val, "tool profile default overridden by env");
            }
        }

        // Per-channel overrides
        let channels = ["telegram", "discord", "slack", "voice", "teams", "signal", "matrix"];
        for channel in &channels {
            let env_key = format!("BEACON_TOOL_PROFILE_{}", channel.to_uppercase());
            if let Ok(val) = std::env::var(&env_key) {
                if let Some(profile) = ToolProfile::from_str_value(&val) {
                    self.channel_profiles.insert((*channel).to_string(), profile);
                    tracing::info!(channel, profile = val, "tool profile overridden by env");
                }
            }
        }

        self
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
    fn from_str_value_parses_all_profiles() {
        assert_eq!(ToolProfile::from_str_value("none"), Some(ToolProfile::None));
        assert_eq!(ToolProfile::from_str_value("minimal"), Some(ToolProfile::Minimal));
        assert_eq!(ToolProfile::from_str_value("messaging"), Some(ToolProfile::Messaging));
        assert_eq!(ToolProfile::from_str_value("full"), Some(ToolProfile::Full));
        assert_eq!(ToolProfile::from_str_value("custom"), Some(ToolProfile::Custom));
        // Case insensitive
        assert_eq!(ToolProfile::from_str_value("Full"), Some(ToolProfile::Full));
        assert_eq!(ToolProfile::from_str_value("MESSAGING"), Some(ToolProfile::Messaging));
        // Invalid
        assert_eq!(ToolProfile::from_str_value("invalid"), None);
        assert_eq!(ToolProfile::from_str_value(""), None);
    }

    #[test]
    fn with_env_overrides_preserves_defaults_when_no_env() {
        // When no BEACON_TOOL_PROFILE* env vars are set, policy remains unchanged
        let original = ToolPolicy::default_policy();
        let with_overrides = ToolPolicy::default_policy().with_env_overrides();

        assert_eq!(
            original.profile_for("voice"),
            with_overrides.profile_for("voice")
        );
        assert_eq!(
            original.profile_for("telegram"),
            with_overrides.profile_for("telegram")
        );
        assert_eq!(
            original.profile_for("unknown"),
            with_overrides.profile_for("unknown")
        );
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
