//! Skill type definitions

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Serde helper for fields that default to `true`
fn default_true() -> bool {
    true
}

/// Skill metadata from SKILL.md frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Always include in system prompt regardless of budget
    #[serde(default)]
    pub always: bool,
    /// Can be invoked as a slash command by the user
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Prevent the model from invoking this skill on its own
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Emoji for display in command listings
    #[serde(default)]
    pub emoji: Option<String>,
    /// Env vars that must be set for this skill to be eligible
    #[serde(default)]
    pub requires_env: Vec<String>,
    /// OS restrictions (e.g. ["linux", "darwin"]). Empty = all platforms
    #[serde(default)]
    pub os: Vec<String>,
    /// All listed binaries must be on PATH for eligibility
    #[serde(default)]
    pub requires_bins: Vec<String>,
    /// At least one listed binary must be on PATH for eligibility
    #[serde(default)]
    pub requires_any_bins: Vec<String>,
    /// Primary env var name for API key injection (e.g. "GITHUB_TOKEN")
    #[serde(default)]
    pub primary_env: Option<String>,
    /// Tool dispatch: "tool" to dispatch slash command to a tool
    #[serde(default, rename = "command-dispatch")]
    pub command_dispatch: Option<String>,
    /// Target tool name for dispatch
    #[serde(default, rename = "command-tool")]
    pub command_tool: Option<String>,
}

/// A loaded skill with content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub metadata: SkillMetadata,
    pub content: String,
    pub source: SkillSource,
}

/// Where the skill was loaded from
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Local filesystem
    Local,
    /// Manifold registry
    Manifold {
        namespace: String,
        repository: String,
    },
    /// Bundled with Beacon
    Bundled,
}

/// Skill priority determines placement in the system prompt hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillPriority {
    /// Placed BEFORE persona — overrides personality/behavior
    Override,
    /// Placed AFTER persona — extends capabilities (default)
    #[default]
    Standard,
    /// Placed at the end — supplementary context/knowledge
    Supplementary,
}

impl SkillPriority {
    /// Parse from database string value
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "override" => Self::Override,
            "supplementary" => Self::Supplementary,
            _ => Self::Standard,
        }
    }

    /// Convert to database string value
    #[must_use]
    pub const fn as_db(&self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Standard => "standard",
            Self::Supplementary => "supplementary",
        }
    }
}

/// Skill installation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub skill: Skill,
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub enabled: bool,
    pub priority: SkillPriority,
    /// Derived slash command name (e.g. "pirate" for `/pirate`)
    pub command_name: Option<String>,
    /// Owner user ID (None = shared/bundled, Some = per-user)
    pub user_id: Option<String>,
    /// Slash command dispatches to this tool instead of injecting into prompt
    pub command_dispatch_tool: Option<String>,
    /// API key value for primary_env injection
    pub api_key: Option<String>,
    /// Custom env var overrides for this skill
    #[serde(default)]
    pub skill_env: HashMap<String, String>,
}

/// Sanitize a skill name into a valid command name
///
/// Non-alphanumeric characters become `_`, lowercased, max 32 chars.
#[must_use]
pub fn sanitize_command_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .take(32)
        .collect();
    sanitized.trim_matches('_').to_string()
}

/// Check if a binary is available on PATH
#[must_use]
pub fn has_binary(name: &str) -> bool {
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&candidate) {
                        if meta.permissions().mode() & 0o111 != 0 {
                            return true;
                        }
                    }
                }
                #[cfg(not(unix))]
                return true;
            }
        }
    }
    false
}

/// Generate a unique command name by appending `_2`, `_3`, etc. if needed
#[must_use]
pub fn deduplicate_command_name(name: &str, existing: &[String]) -> String {
    let base = sanitize_command_name(name);
    if !existing.contains(&base) {
        return base;
    }
    for i in 2..=100 {
        let candidate = format!("{base}_{i}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base}_{}", uuid::Uuid::new_v4().as_simple())
}
