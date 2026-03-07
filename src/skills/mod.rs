//! Skills system for extensible agent capabilities

pub mod install;
mod manifold;
mod types;

pub use manifold::ManifoldClient;
pub use types::{
    InstallKind, InstalledSkill, NodeManager, Skill, SkillFilter, SkillInstallPreferences,
    SkillInstallResult, SkillInstallSpec, SkillMetadata, SkillPriority, SkillSnapshot, SkillSource,
    SnapshotEntry, deduplicate_command_name, has_binary, merge_nested_metadata,
    sanitize_command_name,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Skills compiled into the binary (lowest precedence)
const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "concise",
        "---\nname: concise\ndescription: Keep responses brief and direct\nalways: true\nuser_invocable: false\ntags:\n  - behavior\n---\n\nPrefer short, direct answers. Avoid filler phrases and unnecessary preamble.\n",
    ),
    (
        "summarize",
        concat!(
            "---\nname: summarize\ndescription: Summarize text, articles, or conversations\n",
            "user_invocable: true\ntags:\n  - productivity\n  - writing\n---\n\n",
            "Summarize the provided content. Follow these rules:\n\n",
            "1. Start with a one-sentence TL;DR\n",
            "2. Follow with 3-7 bullet points covering key information\n",
            "3. Preserve factual accuracy — never invent details\n",
            "4. Match the technical level of the source material\n",
            "5. If the content is a conversation, focus on decisions and action items\n",
            "6. If the content is an article, focus on the thesis and supporting evidence\n",
        ),
    ),
    (
        "translate",
        concat!(
            "---\nname: translate\ndescription: Translate text between languages\n",
            "user_invocable: true\ntags:\n  - language\n---\n\n",
            "Translate the provided text. Rules:\n\n",
            "1. If the target language isn't specified, detect the source language and translate to English (or from English to the most likely intended language based on context)\n",
            "2. Preserve tone, formality level, and intent\n",
            "3. For technical terms, provide the translation with the original in parentheses on first occurrence\n",
            "4. For idioms, translate the meaning rather than word-for-word\n",
            "5. If ambiguous, briefly note the alternative interpretation\n",
        ),
    ),
    (
        "code-review",
        concat!(
            "---\nname: code-review\ndescription: Review code for bugs, style, and improvements\n",
            "user_invocable: true\ntags:\n  - development\n---\n\n",
            "Review the provided code. Structure your review as:\n\n",
            "1. **Critical issues** — bugs, security vulnerabilities, data loss risks\n",
            "2. **Improvements** — performance, readability, maintainability\n",
            "3. **Nitpicks** — style, naming, minor suggestions (keep brief)\n\n",
            "Rules:\n",
            "- Be specific: reference line numbers or code snippets\n",
            "- Explain *why* something is problematic, not just *what*\n",
            "- Suggest concrete fixes, not vague advice\n",
            "- Acknowledge what's done well (briefly)\n",
            "- Don't suggest changes that are purely stylistic preference\n",
        ),
    ),
    (
        "explain",
        concat!(
            "---\nname: explain\ndescription: Explain a concept, code, or error clearly\n",
            "user_invocable: true\ntags:\n  - learning\n---\n\n",
            "Explain the topic clearly. Adapt to the user's apparent level:\n\n",
            "1. Start with a one-sentence definition or summary\n",
            "2. Use an analogy if the concept is abstract\n",
            "3. Show a concrete example\n",
            "4. Mention common misconceptions or gotchas\n",
            "5. If explaining code: walk through execution step by step\n",
            "6. If explaining an error: state the cause, then the fix\n",
        ),
    ),
    (
        "meeting-notes",
        concat!(
            "---\nname: meeting-notes\ndescription: Extract structured notes from meeting transcripts\n",
            "user_invocable: true\ntags:\n  - productivity\n---\n\n",
            "Extract structured meeting notes from the provided transcript or conversation.\n\n",
            "Format:\n",
            "## Meeting Notes\n\n",
            "**Date:** [if mentioned]\n",
            "**Attendees:** [if mentioned]\n\n",
            "### Summary\n",
            "[2-3 sentence overview]\n\n",
            "### Key Decisions\n",
            "- [decision] — [context/rationale]\n\n",
            "### Action Items\n",
            "- [ ] [task] — [owner, if mentioned] — [deadline, if mentioned]\n\n",
            "### Open Questions\n",
            "- [unresolved items]\n",
        ),
    ),
    (
        "proofread",
        concat!(
            "---\nname: proofread\ndescription: Fix grammar, spelling, and clarity in text\n",
            "user_invocable: true\ntags:\n  - writing\n---\n\n",
            "Proofread and improve the provided text. Rules:\n\n",
            "1. Fix grammar, spelling, and punctuation errors\n",
            "2. Improve clarity and readability without changing the author's voice\n",
            "3. Remove redundancy and filler\n",
            "4. Preserve technical terms and intentional style choices\n",
            "5. Output the corrected text, then briefly list the changes made\n",
            "6. If the text is already clean, say so\n",
        ),
    ),
    (
        "data-analysis",
        concat!(
            "---\nname: data-analysis\ndescription: Analyze data and generate insights\n",
            "user_invocable: true\ntags:\n  - productivity\n  - analysis\n---\n\n",
            "Analyze the provided data. Approach:\n\n",
            "1. Describe what you see: shape, types, notable patterns\n",
            "2. Identify trends, outliers, and correlations\n",
            "3. Provide specific numbers — don't just say \"increased\", say \"increased 23%\"\n",
            "4. If the data supports it, suggest visualizations (describe chart type and axes)\n",
            "5. State limitations: what the data can and cannot tell us\n",
            "6. End with 2-3 actionable insights or next questions to investigate\n",
        ),
    ),
    (
        "email-draft",
        concat!(
            "---\nname: email-draft\ndescription: Draft professional emails\n",
            "user_invocable: true\ntags:\n  - writing\n  - productivity\n---\n\n",
            "Draft an email based on the user's intent. Rules:\n\n",
            "1. Match formality to the context (colleague vs. client vs. executive)\n",
            "2. Lead with the purpose — no \"I hope this email finds you well\" unless appropriate\n",
            "3. Be specific about asks, deadlines, and next steps\n",
            "4. Keep it as short as possible while being complete\n",
            "5. End with a clear call to action\n",
            "6. Provide a subject line\n",
        ),
    ),
    (
        "debug",
        concat!(
            "---\nname: debug\ndescription: Systematic debugging of errors and unexpected behavior\n",
            "user_invocable: true\ntags:\n  - development\n---\n\n",
            "Debug the issue systematically:\n\n",
            "1. **Reproduce**: Confirm the exact error message, input, and environment\n",
            "2. **Isolate**: Narrow down where the failure occurs (which function, line, or request)\n",
            "3. **Hypothesize**: List 2-3 most likely root causes, ranked by probability\n",
            "4. **Verify**: For each hypothesis, describe what evidence would confirm or rule it out\n",
            "5. **Fix**: Propose the minimal change that addresses the root cause\n",
            "6. **Prevent**: Suggest how to catch this class of bug earlier (test, assertion, type)\n\n",
            "Don't guess. If you need more information, ask specific questions.\n",
        ),
    ),
];

/// Limits applied during directory scanning
#[derive(Debug, Clone)]
pub struct ScanLimits {
    pub max_skill_file_bytes: usize,
    pub max_candidates_per_root: usize,
    pub max_skills_per_source: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_skill_file_bytes: 256_000,
            max_candidates_per_root: 1000,
            max_skills_per_source: 200,
        }
    }
}

impl ScanLimits {
    /// Build from a `SkillsConfig`
    #[must_use]
    pub const fn from_config(config: &crate::config::SkillsConfig) -> Self {
        Self {
            max_skill_file_bytes: config.max_skill_file_bytes,
            max_candidates_per_root: config.max_candidates_per_root,
            max_skills_per_source: config.max_skills_per_source,
        }
    }
}

/// Skill registry for discovery and management
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    cache_dir: PathBuf,
}

impl SkillRegistry {
    /// Create a new skill registry
    #[must_use]
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            skills: HashMap::new(),
            cache_dir,
        }
    }

    /// Discover all skills: bundled (lowest precedence) then managed directories
    ///
    /// Managed directory skills override bundled skills by name.
    ///
    /// # Errors
    ///
    /// Returns an error if a directory cannot be read
    pub fn discover_all(&mut self, managed_dirs: &[PathBuf]) -> Result<usize> {
        let mut count = 0;

        // Load bundled skills first (lowest precedence)
        count += self.load_bundled(&[]);

        // Load managed directory skills (override bundled by name)
        for dir in managed_dirs {
            if !dir.is_dir() {
                continue;
            }
            count += self.scan_directory(dir)?;
        }

        Ok(count)
    }

    /// Discover skills from all roots with precedence ordering
    ///
    /// Precedence (later overrides earlier by name):
    /// 1. Bundled (filtered by `allow_bundled`)
    /// 2. Extra dirs
    /// 3. Managed dir
    /// 4. Personal agent dir (`~/.agents/skills/`)
    ///
    /// # Errors
    ///
    /// Returns an error if a directory cannot be read
    pub fn discover_all_roots(&mut self, config: &crate::config::SkillsConfig) -> Result<usize> {
        let mut count = 0;
        let limits = ScanLimits::from_config(config);

        // 1. Bundled (filtered by allowlist)
        count += self.load_bundled(&config.allow_bundled);

        // 2. Extra dirs
        for dir in &config.extra_dirs {
            if dir.is_dir() {
                count += self.scan_directory_with_limits(dir, &limits)?;
            }
        }

        // 3. Managed dir
        if config.managed_dir.is_dir() {
            count += self.scan_directory_with_limits(&config.managed_dir, &limits)?;
        }

        // 4. Personal agent dir
        if config.personal_dir.is_dir() {
            count += self.scan_directory_with_limits(&config.personal_dir, &limits)?;
        }

        Ok(count)
    }

    /// Scan plugin skill directories, tagging discoveries as `SkillSource::Plugin`
    ///
    /// # Errors
    ///
    /// Returns an error if a directory cannot be read
    pub fn scan_plugin_dirs(
        &mut self,
        dirs: &[PathBuf],
        config: &crate::config::SkillsConfig,
    ) -> Result<usize> {
        let limits = ScanLimits::from_config(config);
        let mut count = 0;
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            count += self.scan_directory_with_source(dir, &limits, &SkillSource::Plugin)?;
        }
        Ok(count)
    }

    /// Load bundled skills, optionally filtered by allowlist
    fn load_bundled(&mut self, allow_bundled: &[String]) -> usize {
        let mut count = 0;
        for (name, raw) in BUNDLED_SKILLS {
            // If allowlist is non-empty, skip skills not in it
            if !allow_bundled.is_empty() && !allow_bundled.iter().any(|a| a == name) {
                tracing::debug!(name, "bundled skill filtered by allowlist");
                continue;
            }
            match parse_frontmatter(raw) {
                Ok((metadata, body)) => {
                    let skill = Skill {
                        id: (*name).to_string(),
                        metadata,
                        content: body,
                        source: SkillSource::Bundled,
                        location: None,
                    };
                    self.skills.insert(skill.id.clone(), skill);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(name, error = %e, "failed to parse bundled skill");
                }
            }
        }
        count
    }

    /// Discover skills from local directories
    ///
    /// # Errors
    ///
    /// Returns an error if a directory cannot be read
    pub fn discover_local(&mut self, dirs: &[PathBuf]) -> Result<usize> {
        let mut count = 0;
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            count += self.scan_directory(dir)?;
        }
        Ok(count)
    }

    /// Scan a directory for SKILL.md files with safety limits
    fn scan_directory(&mut self, dir: &Path) -> Result<usize> {
        self.scan_directory_with_limits(dir, &ScanLimits::default())
    }

    /// Scan a directory for SKILL.md files with explicit safety limits
    fn scan_directory_with_limits(&mut self, dir: &Path, limits: &ScanLimits) -> Result<usize> {
        self.scan_directory_with_source(dir, limits, &SkillSource::Local)
    }

    /// Scan a directory with explicit limits and source tag
    fn scan_directory_with_source(
        &mut self,
        dir: &Path,
        limits: &ScanLimits,
        source: &SkillSource,
    ) -> Result<usize> {
        let count = self.scan_directory_inner(dir, limits, source)?;

        // Nested root detection: if zero skills found, check for `dir/skills/`
        if count == 0 {
            let nested = dir.join("skills");
            if nested.is_dir() {
                tracing::debug!(
                    parent = %dir.display(),
                    nested = %nested.display(),
                    "no skills found in root, trying nested skills/ dir"
                );
                return self.scan_directory_inner(&nested, limits, source);
            }
        }

        Ok(count)
    }

    /// Inner scan with limits enforcement
    fn scan_directory_inner(
        &mut self,
        dir: &Path,
        limits: &ScanLimits,
        source: &SkillSource,
    ) -> Result<usize> {
        let mut count = 0;
        let mut candidates_scanned = 0;
        let entries = std::fs::read_dir(dir).map_err(|e| Error::Skill(e.to_string()))?;

        for entry in entries.flatten() {
            if candidates_scanned >= limits.max_candidates_per_root {
                tracing::warn!(
                    dir = %dir.display(),
                    limit = limits.max_candidates_per_root,
                    "max candidates per root reached, stopping scan"
                );
                break;
            }

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            candidates_scanned += 1;

            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }

            // File size check
            #[allow(clippy::cast_possible_truncation)]
            // file size fits in usize on supported platforms
            if let Ok(meta) = std::fs::metadata(&skill_file)
                && meta.len() as usize > limits.max_skill_file_bytes
            {
                tracing::warn!(
                    path = %skill_file.display(),
                    size = meta.len(),
                    limit = limits.max_skill_file_bytes,
                    "skill file exceeds size limit, skipping"
                );
                continue;
            }

            if count >= limits.max_skills_per_source {
                tracing::warn!(
                    dir = %dir.display(),
                    limit = limits.max_skills_per_source,
                    "max skills per source reached, stopping scan"
                );
                break;
            }

            match load_skill_file_with_source(&skill_file, source.clone()) {
                Ok(skill) => {
                    self.skills.insert(skill.id.clone(), skill);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %skill_file.display(), error = %e, "failed to load skill");
                }
            }
        }
        Ok(count)
    }

    /// Get a skill by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.skills.get(id)
    }

    /// List all skills
    #[must_use]
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// Check if a skill exists
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.skills.contains_key(id)
    }

    /// Get the cache directory
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

/// Load a skill from a SKILL.md file with an explicit source tag
fn load_skill_file_with_source(path: &Path, source: SkillSource) -> Result<Skill> {
    let content = std::fs::read_to_string(path).map_err(|e| Error::Skill(e.to_string()))?;

    let (metadata, body) = parse_frontmatter(&content)?;

    let id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut skill = Skill {
        id,
        metadata,
        content: body,
        source,
        location: None,
    };

    skill.location = Some(crate::prompt::compact_path(&path.to_string_lossy()));

    Ok(skill)
}

/// Parse YAML frontmatter from markdown
fn parse_frontmatter(content: &str) -> Result<(SkillMetadata, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err(Error::Skill("missing frontmatter".to_string()));
    }

    let rest = &content[3..];
    let end = rest
        .find("---")
        .ok_or_else(|| Error::Skill("unclosed frontmatter".to_string()))?;

    let frontmatter = &rest[..end];
    let body = rest[end + 3..].trim().to_string();

    let mut metadata: SkillMetadata =
        serde_yaml::from_str(frontmatter).map_err(|e| Error::Skill(e.to_string()))?;

    // Merge nested metadata (fills defaults only, flat fields win)
    if let Ok(raw) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) {
        types::merge_nested_metadata(&mut metadata, &raw);
    }

    Ok((metadata, body))
}

/// What to do when a slash command is resolved
#[derive(Debug)]
pub enum SlashCommandAction {
    /// Inject skill content into system prompt (existing behavior)
    InjectPrompt {
        skill: InstalledSkill,
        remaining: String,
    },
    /// Dispatch directly to a tool
    DispatchTool {
        skill: InstalledSkill,
        tool_name: String,
        arguments: String,
    },
}

/// Resolve a slash command from user input
///
/// Parses `/command_name` from the start of input, looks up via `SkillRepo`,
/// and returns the matching action if found.
///
/// # Errors
///
/// Returns error if database lookup fails
pub fn resolve_slash_command(
    input: &str,
    skill_repo: &crate::db::SkillRepo,
    user_id: Option<&str>,
) -> Result<Option<SlashCommandAction>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    // Extract command name (first word after /)
    let after_slash = &trimmed[1..];
    let command_end = after_slash
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_slash.len());
    let command = &after_slash[..command_end];

    if command.is_empty() {
        return Ok(None);
    }

    // Look up in database
    let skill = skill_repo.get_by_command_name(command, user_id)?;

    match skill {
        Some(s) if s.enabled && s.skill.metadata.user_invocable => {
            let remaining = after_slash[command_end..].trim().to_string();

            if let Some(ref tool_name) = s.command_dispatch_tool {
                Ok(Some(SlashCommandAction::DispatchTool {
                    tool_name: tool_name.clone(),
                    arguments: remaining,
                    skill: s,
                }))
            } else {
                Ok(Some(SlashCommandAction::InjectPrompt {
                    skill: s,
                    remaining,
                }))
            }
        }
        _ => Ok(None),
    }
}

/// Sync discovered skills into the database at startup
///
/// - Bundled skills: upsert content (preserve user's enabled/priority)
/// - Managed/local skills: install if not already present by name
/// - Generates `command_name` for user-invocable skills
///
/// # Errors
///
/// Returns error if database operations fail
pub fn sync_discovered_skills(
    skill_repo: &crate::db::SkillRepo,
    registry: &SkillRegistry,
) -> Result<usize> {
    let mut synced = 0;
    for skill in registry.skills.values() {
        match skill.source {
            SkillSource::Bundled => {
                skill_repo.upsert_bundled(skill, SkillPriority::Standard)?;
                synced += 1;
            }
            SkillSource::Local | SkillSource::Manifold { .. } | SkillSource::Plugin => {
                // Only install if not already present
                if skill_repo.get_by_name(&skill.metadata.name)?.is_none() {
                    skill_repo.install(skill)?;
                    synced += 1;
                }
            }
        }
    }

    if synced > 0 {
        tracing::info!(count = synced, "synced discovered skills to database");
    }

    Ok(synced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_works() {
        let content = r"---
name: test-skill
description: A test skill
tags:
  - testing
---

# Test Skill

This is the content.
";
        let (metadata, body) = parse_frontmatter(content).unwrap();
        assert_eq!(metadata.name, "test-skill");
        assert_eq!(metadata.description, "A test skill");
        assert!(body.contains("# Test Skill"));
    }

    #[test]
    fn parse_frontmatter_missing_fails() {
        let content = "# No frontmatter";
        assert!(parse_frontmatter(content).is_err());
    }

    #[test]
    fn parse_nested_metadata_format() {
        let content = r#"---
name: nested-skill
description: A skill with nested metadata
metadata:
  openclaw:
    requires:
      env: [OPENAI_API_KEY]
      bins: [jq]
      anyBins: [gh, hub]
      config: [voice.enabled]
    primaryEnv: OPENAI_API_KEY
    always: true
    emoji: "\U0001F916"
    os: [linux, darwin]
    install:
      - kind: brew
        formula: jq
        bins: [jq]
---

Nested skill body.
"#;
        let (meta, body) = parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "nested-skill");
        assert_eq!(meta.requires_env, vec!["OPENAI_API_KEY"]);
        assert_eq!(meta.requires_bins, vec!["jq"]);
        assert_eq!(meta.requires_any_bins, vec!["gh", "hub"]);
        assert_eq!(meta.requires_config, vec!["voice.enabled"]);
        assert_eq!(meta.primary_env.as_deref(), Some("OPENAI_API_KEY"));
        assert!(meta.always);
        assert_eq!(meta.emoji.as_deref(), Some("\u{1F916}"));
        assert_eq!(meta.os, vec!["linux", "darwin"]);
        assert_eq!(meta.install.len(), 1);
        assert_eq!(meta.install[0].formula.as_deref(), Some("jq"));
        assert!(body.contains("Nested skill body."));
    }

    #[test]
    fn parse_nested_alias_clawdbot() {
        let content = "---\nname: alias-skill\ndescription: Uses clawdbot alias\nmetadata:\n  clawdbot:\n    requires:\n      env: [MY_TOKEN]\n    emoji: \"\u{1F43E}\"\n---\n\nBody.\n";
        let (meta, _) = parse_frontmatter(content).unwrap();
        assert_eq!(meta.requires_env, vec!["MY_TOKEN"]);
        assert_eq!(meta.emoji.as_deref(), Some("\u{1F43E}"));
    }

    #[test]
    fn flat_fields_override_nested() {
        let content = r"---
name: override-test
description: Flat wins over nested
requires_env: [FLAT_TOKEN]
emoji: flat-emoji
metadata:
  openclaw:
    requires:
      env: [NESTED_TOKEN]
    emoji: nested-emoji
---

Body.
";
        let (meta, _) = parse_frontmatter(content).unwrap();
        assert_eq!(meta.requires_env, vec!["FLAT_TOKEN"]);
        assert_eq!(meta.emoji.as_deref(), Some("flat-emoji"));
    }

    #[test]
    fn mixed_flat_and_nested() {
        let content = r"---
name: mixed-test
description: Some flat some nested
always: true
metadata:
  openclaw:
    requires:
      env: [NESTED_TOKEN]
      bins: [curl]
    primaryEnv: NESTED_TOKEN
---

Body.
";
        let (meta, _) = parse_frontmatter(content).unwrap();
        // `always` set flat
        assert!(meta.always);
        // These come from nested
        assert_eq!(meta.requires_env, vec!["NESTED_TOKEN"]);
        assert_eq!(meta.requires_bins, vec!["curl"]);
        assert_eq!(meta.primary_env.as_deref(), Some("NESTED_TOKEN"));
    }

    #[test]
    fn pure_beacon_format_unchanged() {
        let content = "---\nname: beacon-skill\ndescription: A pure Beacon skill\nrequires_env: [MY_KEY]\nrequires_bins: [git]\nprimary_env: MY_KEY\nalways: true\nemoji: \"\u{1F680}\"\nos: [linux]\n---\n\nBeacon body.\n";
        let (meta, body) = parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "beacon-skill");
        assert_eq!(meta.requires_env, vec!["MY_KEY"]);
        assert_eq!(meta.requires_bins, vec!["git"]);
        assert_eq!(meta.primary_env.as_deref(), Some("MY_KEY"));
        assert!(meta.always);
        assert_eq!(meta.emoji.as_deref(), Some("\u{1F680}"));
        assert_eq!(meta.os, vec!["linux"]);
        assert!(body.contains("Beacon body."));
    }
}
