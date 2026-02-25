//! Skill type definitions

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Serde helper for fields that default to `true`
fn default_true() -> bool {
    true
}

/// How a skill dependency should be installed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    Brew,
    Node,
    Go,
    Uv,
    Download,
}

/// A single install specification for a skill dependency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallSpec {
    pub kind: InstallKind,
    #[serde(default)]
    pub label: Option<String>,
    /// Expected binaries after install
    #[serde(default)]
    pub bins: Vec<String>,
    /// Per-spec OS filter
    #[serde(default)]
    pub os: Vec<String>,
    /// Homebrew formula
    #[serde(default)]
    pub formula: Option<String>,
    /// Node / uv package name
    #[serde(default)]
    pub package: Option<String>,
    /// Go module path
    #[serde(default)]
    pub module: Option<String>,
    /// Download URL
    #[serde(default)]
    pub url: Option<String>,
    /// Archive format: "tar.gz", "tar.bz2", "zip"
    #[serde(default)]
    pub archive: Option<String>,
    /// Strip leading path components when extracting
    #[serde(default)]
    pub strip_components: Option<u32>,
    /// Target directory for extracted binaries
    #[serde(default)]
    pub target_dir: Option<String>,
}

/// Preferred Node.js package manager
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeManager {
    #[default]
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

/// User preferences for install automation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallPreferences {
    #[serde(default = "default_true")]
    pub prefer_brew: bool,
    #[serde(default)]
    pub node_manager: NodeManager,
}

impl Default for SkillInstallPreferences {
    fn default() -> Self {
        Self {
            prefer_brew: true,
            node_manager: NodeManager::default(),
        }
    }
}

/// Result of executing an install command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallResult {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    pub code: Option<i32>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Filter for agent-level skill visibility
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFilter {
    /// Patterns to include (empty = all)
    #[serde(default)]
    pub include: Vec<String>,
    /// Patterns to exclude
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl SkillFilter {
    /// Check whether a skill name passes this filter
    #[must_use]
    pub fn allows(&self, name: &str) -> bool {
        let included = self.include.is_empty()
            || self.include.iter().any(|p| pattern_matches(p, name));
        let excluded = self.exclude.iter().any(|p| pattern_matches(p, name));
        included && !excluded
    }
}

/// Match a pattern against a name (supports `*`, `prefix*`, `*suffix`, exact)
fn pattern_matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    pattern == name
}

/// Workspace snapshot of all installed skills
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSnapshot {
    pub version: String,
    pub created_at: String,
    pub skills: Vec<SnapshotEntry>,
}

/// A single skill entry within a snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub name: String,
    pub version: Option<String>,
    pub source: SkillSource,
    pub enabled: bool,
    pub priority: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub skill_env: HashMap<String, String>,
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
    /// Install automation specs for dependencies
    #[serde(default)]
    pub install: Vec<SkillInstallSpec>,
    /// Config paths that must be satisfied for eligibility (e.g. "voice.enabled")
    #[serde(default)]
    pub requires_config: Vec<String>,
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
    /// Loaded from a plugin
    Plugin,
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

/// Merge nested skill metadata into flat Beacon fields
///
/// Checks `metadata.openclaw`, `metadata.clawdbot`, and `metadata.clawdis`
/// for nested runtime fields and fills in any Beacon fields still at defaults.
/// Flat fields always win — the merge only fills in defaults.
pub fn merge_nested_metadata(meta: &mut SkillMetadata, raw: &serde_yaml::Value) {
    let metadata_map = match raw.get("metadata") {
        Some(m) => m,
        None => return,
    };

    // Check all three nested metadata alias keys
    let oc = ["openclaw", "clawdbot", "clawdis"]
        .iter()
        .find_map(|key| metadata_map.get(*key));

    let oc = match oc {
        Some(v) => v,
        None => return,
    };

    // requires.env → requires_env
    if meta.requires_env.is_empty() {
        if let Some(vals) = oc.get("requires").and_then(|r| r.get("env")).and_then(|v| v.as_sequence()) {
            meta.requires_env = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
    }

    // requires.bins → requires_bins
    if meta.requires_bins.is_empty() {
        if let Some(vals) = oc.get("requires").and_then(|r| r.get("bins")).and_then(|v| v.as_sequence()) {
            meta.requires_bins = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
    }

    // requires.anyBins → requires_any_bins
    if meta.requires_any_bins.is_empty() {
        if let Some(vals) = oc.get("requires").and_then(|r| r.get("anyBins")).and_then(|v| v.as_sequence()) {
            meta.requires_any_bins = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
    }

    // requires.config → requires_config
    if meta.requires_config.is_empty() {
        if let Some(vals) = oc.get("requires").and_then(|r| r.get("config")).and_then(|v| v.as_sequence()) {
            meta.requires_config = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
    }

    // primaryEnv → primary_env
    if meta.primary_env.is_none() {
        if let Some(val) = oc.get("primaryEnv").and_then(|v| v.as_str()) {
            meta.primary_env = Some(val.to_string());
        }
    }

    // always (only override if still at default false)
    if !meta.always {
        if let Some(val) = oc.get("always").and_then(|v| v.as_bool()) {
            meta.always = val;
        }
    }

    // emoji
    if meta.emoji.is_none() {
        if let Some(val) = oc.get("emoji").and_then(|v| v.as_str()) {
            meta.emoji = Some(val.to_string());
        }
    }

    // os
    if meta.os.is_empty() {
        if let Some(vals) = oc.get("os").and_then(|v| v.as_sequence()) {
            meta.os = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
    }

    // install
    if meta.install.is_empty() {
        if let Some(install_val) = oc.get("install") {
            if let Ok(specs) = serde_yaml::from_value::<Vec<SkillInstallSpec>>(install_val.clone()) {
                meta.install = specs;
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_empty_allows_all() {
        let filter = SkillFilter::default();
        assert!(filter.allows("anything"));
        assert!(filter.allows("weather"));
    }

    #[test]
    fn filter_include_only() {
        let filter = SkillFilter {
            include: vec!["weather".to_string(), "clock".to_string()],
            exclude: vec![],
        };
        assert!(filter.allows("weather"));
        assert!(filter.allows("clock"));
        assert!(!filter.allows("pirate"));
    }

    #[test]
    fn filter_exclude() {
        let filter = SkillFilter {
            include: vec![],
            exclude: vec!["pirate".to_string()],
        };
        assert!(filter.allows("weather"));
        assert!(!filter.allows("pirate"));
    }

    #[test]
    fn filter_glob() {
        let filter = SkillFilter {
            include: vec!["dev-*".to_string()],
            exclude: vec!["*-beta".to_string()],
        };
        assert!(filter.allows("dev-tools"));
        assert!(filter.allows("dev-lint"));
        assert!(!filter.allows("prod-tools"));
        assert!(!filter.allows("dev-beta")); // excluded by *-beta
    }

    #[test]
    fn filter_wildcard_include() {
        let filter = SkillFilter {
            include: vec!["*".to_string()],
            exclude: vec!["secret".to_string()],
        };
        assert!(filter.allows("anything"));
        assert!(!filter.allows("secret"));
    }

    #[test]
    fn pattern_matches_exact() {
        assert!(pattern_matches("hello", "hello"));
        assert!(!pattern_matches("hello", "world"));
    }

    #[test]
    fn pattern_matches_prefix() {
        assert!(pattern_matches("dev-*", "dev-tools"));
        assert!(!pattern_matches("dev-*", "prod-tools"));
    }

    #[test]
    fn pattern_matches_suffix() {
        assert!(pattern_matches("*-beta", "tool-beta"));
        assert!(!pattern_matches("*-beta", "tool-release"));
    }
}
