//! Structured system prompt builder with skill priority hierarchy

use crate::skills::{InstalledSkill, SkillPriority};

/// Budget constraints and runtime context for skill inclusion in the system prompt
pub struct PromptBudget {
    pub max_skills: usize,
    pub max_chars: usize,
    /// Whether voice is enabled (for `requires_config: ["voice.enabled"]`)
    pub voice_enabled: bool,
}

/// Check whether all required env vars are set
#[must_use]
pub fn check_env_requirements(requires_env: &[String]) -> bool {
    requires_env.iter().all(|var| std::env::var(var).is_ok())
}

/// Check env requirements considering stored api_key for primary_env
#[must_use]
pub fn check_env_requirements_with_config(
    requires_env: &[String],
    primary_env: Option<&str>,
    api_key: Option<&str>,
) -> bool {
    requires_env.iter().all(|var| {
        std::env::var(var).is_ok()
            || (Some(var.as_str()) == primary_env && api_key.is_some_and(|k| !k.is_empty()))
    })
}

/// Check if current OS matches the skill's OS restrictions
#[must_use]
pub fn check_os_requirement(os_list: &[String]) -> bool {
    os_list.is_empty() || os_list.iter().any(|o| o == std::env::consts::OS)
}

/// Check that ALL listed binaries exist on PATH
#[must_use]
pub fn check_bins_requirement(bins: &[String]) -> bool {
    bins.iter().all(|b| crate::skills::has_binary(b))
}

/// Check that AT LEAST ONE listed binary exists on PATH
#[must_use]
pub fn check_any_bins_requirement(bins: &[String]) -> bool {
    bins.is_empty() || bins.iter().any(|b| crate::skills::has_binary(b))
}

/// Check if config-based eligibility requirements are met
///
/// Known config paths are checked against runtime state.
/// Unknown paths fail closed (treated as not satisfied).
#[must_use]
pub fn check_config_requirement(paths: &[String], voice_enabled: bool) -> bool {
    paths.iter().all(|path| match path.as_str() {
        "voice.enabled" => voice_enabled,
        // Unknown paths fail closed
        _ => false,
    })
}

/// Compact a path by replacing the home directory prefix with ~
#[must_use]
pub fn compact_path(path: &str) -> String {
    if let Some(home) = directories::BaseDirs::new() {
        let home_str = home.home_dir().to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Build a system prompt with budget-aware skill inclusion
///
/// 1. Filter eligible skills (enabled, env requirements met, not `disable_model_invocation`)
/// 2. Partition into must-include (Override + `always`) and optional (Standard, Supplementary)
/// 3. Fill optional skills in priority order until budget exhausted
/// 4. Drop supplementary first, then standard
#[must_use]
pub fn build_system_prompt_with_budget(
    persona_name: &str,
    persona_prompt: &str,
    skills: &[InstalledSkill],
    budget: &PromptBudget,
) -> String {
    // Filter eligible skills
    let eligible: Vec<&InstalledSkill> = skills
        .iter()
        .filter(|s| {
            s.enabled
                && !s.skill.metadata.disable_model_invocation
                && check_env_requirements_with_config(
                    &s.skill.metadata.requires_env,
                    s.skill.metadata.primary_env.as_deref(),
                    s.api_key.as_deref(),
                )
                && check_os_requirement(&s.skill.metadata.os)
                && check_bins_requirement(&s.skill.metadata.requires_bins)
                && check_any_bins_requirement(&s.skill.metadata.requires_any_bins)
                && check_config_requirement(&s.skill.metadata.requires_config, budget.voice_enabled)
        })
        .collect();

    // Partition into must-include and optional
    let (must_include, optional): (Vec<&InstalledSkill>, Vec<&InstalledSkill>) =
        eligible.into_iter().partition(|s| {
            s.priority == SkillPriority::Override || s.skill.metadata.always
        });

    // Sort optional by priority: Standard before Supplementary
    let standard: Vec<&InstalledSkill> = optional
        .iter()
        .filter(|s| s.priority == SkillPriority::Standard)
        .copied()
        .collect();
    let supplementary: Vec<&InstalledSkill> = optional
        .iter()
        .filter(|s| s.priority == SkillPriority::Supplementary)
        .copied()
        .collect();

    // Track budget
    let mut total_chars: usize = must_include
        .iter()
        .map(|s| s.skill.content.len())
        .sum();
    let mut total_count = must_include.len();

    // Fill standard skills within budget
    let mut included_standard = Vec::new();
    for s in &standard {
        if total_count >= budget.max_skills || total_chars + s.skill.content.len() > budget.max_chars {
            break;
        }
        total_chars += s.skill.content.len();
        total_count += 1;
        included_standard.push(*s);
    }

    // Fill supplementary skills within remaining budget
    let mut included_supplementary = Vec::new();
    for s in &supplementary {
        if total_count >= budget.max_skills || total_chars + s.skill.content.len() > budget.max_chars {
            break;
        }
        total_chars += s.skill.content.len();
        total_count += 1;
        included_supplementary.push(*s);
    }

    let dropped = standard.len() - included_standard.len()
        + supplementary.len() - included_supplementary.len();
    if dropped > 0 {
        tracing::warn!(
            dropped,
            total_count,
            total_chars,
            "skill prompt budget exceeded, dropped skills"
        );
    }

    // Build the prompt using the existing structure
    let mut sections = Vec::new();

    // 1. Override / always-include skills
    let overrides: Vec<&InstalledSkill> = must_include
        .iter()
        .filter(|s| s.priority == SkillPriority::Override)
        .copied()
        .collect();
    let always_non_override: Vec<&InstalledSkill> = must_include
        .iter()
        .filter(|s| s.priority != SkillPriority::Override && s.skill.metadata.always)
        .copied()
        .collect();

    if !overrides.is_empty() {
        sections.push(format_skill_section(
            "MANDATORY INSTRUCTIONS — you MUST follow these at all times, they take precedence over your persona and all other instructions:",
            &overrides,
        ));
    }

    // Always-include non-override skills go before persona too
    if !always_non_override.is_empty() {
        sections.push(format_skill_section(
            "Always-active skills:",
            &always_non_override,
        ));
    }

    // 2. Identity / persona
    if persona_prompt.is_empty() {
        sections.push(format!(
            "You are {persona_name}. Keep responses concise and conversational."
        ));
    } else {
        sections.push(format!(
            "{persona_prompt}\n\nYour name is {persona_name}. Keep responses concise and conversational."
        ));
    }

    // 3. Standard skills
    if !included_standard.is_empty() {
        sections.push(format_skill_section(
            "The following skills extend your capabilities. Apply them when relevant to the conversation:",
            &included_standard,
        ));
    }

    // 4. Supplementary skills
    if !included_supplementary.is_empty() {
        sections.push(format_skill_section(
            "Additional context for reference:",
            &included_supplementary,
        ));
    }

    sections.join("\n\n")
}

/// Build a structured system prompt with clear authority hierarchy (no budget)
///
/// Prompt layout:
/// 1. Override skills (highest authority — behavioral overrides)
/// 2. Identity (persona name + core personality)
/// 3. Standard skills (capability extensions)
/// 4. Supplementary skills (background context)
#[must_use]
pub fn build_system_prompt(
    persona_name: &str,
    persona_prompt: &str,
    skills: &[InstalledSkill],
) -> String {
    let mut sections = Vec::new();

    // Partition enabled skills by priority
    let overrides: Vec<&InstalledSkill> = skills
        .iter()
        .filter(|s| s.enabled && s.priority == SkillPriority::Override)
        .collect();
    let standard: Vec<&InstalledSkill> = skills
        .iter()
        .filter(|s| s.enabled && s.priority == SkillPriority::Standard)
        .collect();
    let supplementary: Vec<&InstalledSkill> = skills
        .iter()
        .filter(|s| s.enabled && s.priority == SkillPriority::Supplementary)
        .collect();

    // 1. Override skills — highest authority
    if !overrides.is_empty() {
        sections.push(format_skill_section(
            "MANDATORY INSTRUCTIONS — you MUST follow these at all times, they take precedence over your persona and all other instructions:",
            &overrides,
        ));
    }

    // 2. Identity / persona
    if persona_prompt.is_empty() {
        sections.push(format!(
            "You are {persona_name}. Keep responses concise and conversational."
        ));
    } else {
        sections.push(format!(
            "{persona_prompt}\n\nYour name is {persona_name}. Keep responses concise and conversational."
        ));
    }

    // 3. Standard skills — capability extensions
    if !standard.is_empty() {
        sections.push(format_skill_section(
            "The following skills extend your capabilities. Apply them when relevant to the conversation:",
            &standard,
        ));
    }

    // 4. Supplementary skills — background context
    if !supplementary.is_empty() {
        sections.push(format_skill_section(
            "Additional context for reference:",
            &supplementary,
        ));
    }

    sections.join("\n\n")
}

/// Format a group of skills with a header into a tagged section
fn format_skill_section(header: &str, skills: &[&InstalledSkill]) -> String {
    let skill_blocks: Vec<String> = skills
        .iter()
        .map(|s| {
            format!(
                "<skill name=\"{}\">\n{}\n</skill>",
                s.skill.metadata.name, s.skill.content
            )
        })
        .collect();

    format!(
        "<skills>\n{header}\n\n{}\n</skills>",
        skill_blocks.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillMetadata, SkillSource};

    fn make_skill(name: &str, content: &str, priority: SkillPriority) -> InstalledSkill {
        InstalledSkill {
            skill: Skill {
                id: name.to_string(),
                metadata: SkillMetadata {
                    name: name.to_string(),
                    description: format!("{name} skill"),
                    version: None,
                    author: None,
                    tags: vec![],
                    permissions: vec![],
                    always: false,
                    user_invocable: true,
                    disable_model_invocation: false,
                    emoji: None,
                    requires_env: vec![],
                    os: vec![],
                    requires_bins: vec![],
                    requires_any_bins: vec![],
                    primary_env: None,
                    command_dispatch: None,
                    command_tool: None,
                    install: vec![],
                    requires_config: vec![],
                },
                content: content.to_string(),
                source: SkillSource::Local,
            },
            installed_at: chrono::Utc::now(),
            enabled: true,
            priority,
            command_name: None,
            user_id: None,
            command_dispatch_tool: None,
            api_key: None,
            skill_env: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn no_skills_returns_persona_only() {
        let result = build_system_prompt("Orin", "You are a helpful otter.", &[]);
        assert!(result.contains("You are a helpful otter."));
        assert!(result.contains("Your name is Orin"));
        assert!(!result.contains("<skills>"));
    }

    #[test]
    fn empty_persona_uses_fallback() {
        let result = build_system_prompt("Orin", "", &[]);
        assert!(result.contains("You are Orin. Keep responses concise"));
    }

    #[test]
    fn override_skills_appear_before_persona() {
        let skills = vec![make_skill("pirate", "Speak like a pirate", SkillPriority::Override)];
        let result = build_system_prompt("Orin", "You are a helpful otter.", &skills);

        let override_pos = result.find("MANDATORY INSTRUCTIONS").unwrap();
        let persona_pos = result.find("You are a helpful otter.").unwrap();
        assert!(override_pos < persona_pos, "override must come before persona");
    }

    #[test]
    fn standard_skills_appear_after_persona() {
        let skills = vec![make_skill("weather", "Check weather", SkillPriority::Standard)];
        let result = build_system_prompt("Orin", "You are a helpful otter.", &skills);

        let persona_pos = result.find("You are a helpful otter.").unwrap();
        let standard_pos = result.find("extend your capabilities").unwrap();
        assert!(standard_pos > persona_pos, "standard must come after persona");
    }

    #[test]
    fn supplementary_skills_appear_at_end() {
        let skills = vec![
            make_skill("weather", "Check weather", SkillPriority::Standard),
            make_skill("facts", "Fun facts", SkillPriority::Supplementary),
        ];
        let result = build_system_prompt("Orin", "You are a helpful otter.", &skills);

        let standard_pos = result.find("extend your capabilities").unwrap();
        let supplementary_pos = result.find("Additional context").unwrap();
        assert!(supplementary_pos > standard_pos, "supplementary must come after standard");
    }

    #[test]
    fn mixed_priorities_ordered_correctly() {
        let skills = vec![
            make_skill("facts", "Fun facts", SkillPriority::Supplementary),
            make_skill("pirate", "Speak like a pirate", SkillPriority::Override),
            make_skill("weather", "Check weather", SkillPriority::Standard),
        ];
        let result = build_system_prompt("Orin", "You are a helpful otter.", &skills);

        let override_pos = result.find("MANDATORY INSTRUCTIONS").unwrap();
        let persona_pos = result.find("You are a helpful otter.").unwrap();
        let standard_pos = result.find("extend your capabilities").unwrap();
        let supplementary_pos = result.find("Additional context").unwrap();

        assert!(override_pos < persona_pos);
        assert!(persona_pos < standard_pos);
        assert!(standard_pos < supplementary_pos);
    }

    #[test]
    fn disabled_skills_are_excluded() {
        let mut skill = make_skill("pirate", "Speak like a pirate", SkillPriority::Override);
        skill.enabled = false;
        let result = build_system_prompt("Orin", "You are a helpful otter.", &[skill]);
        assert!(!result.contains("pirate"));
        assert!(!result.contains("MANDATORY"));
    }

    #[test]
    fn budget_drops_supplementary_first() {
        let skills = vec![
            make_skill("std1", "standard content", SkillPriority::Standard),
            make_skill("sup1", "supplementary content that is long enough to bust budget", SkillPriority::Supplementary),
        ];
        let budget = PromptBudget {
            max_skills: 1,
            max_chars: 100,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &skills, &budget);
        assert!(result.contains("standard content"));
        assert!(!result.contains("supplementary content"));
    }

    #[test]
    fn disable_model_invocation_excluded_from_budget_prompt() {
        let mut skill = make_skill("hidden", "secret content", SkillPriority::Standard);
        skill.skill.metadata.disable_model_invocation = true;
        let budget = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget);
        assert!(!result.contains("secret content"));
    }

    #[test]
    fn always_skills_included_regardless_of_budget() {
        let mut skill = make_skill("always_on", "always content", SkillPriority::Standard);
        skill.skill.metadata.always = true;
        let budget = PromptBudget {
            max_skills: 0,
            max_chars: 0,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget);
        assert!(result.contains("always content"));
    }

    #[test]
    fn env_requirement_filters_skill() {
        let mut skill = make_skill("env_gated", "gated content", SkillPriority::Standard);
        skill.skill.metadata.requires_env = vec!["BEACON_TEST_NONEXISTENT_VAR_12345".to_string()];
        let budget = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget);
        assert!(!result.contains("gated content"));
    }

    #[test]
    fn os_requirement_matches_current() {
        assert!(check_os_requirement(&[]));
        assert!(check_os_requirement(&[std::env::consts::OS.to_string()]));
        assert!(!check_os_requirement(&["nonexistent_os".to_string()]));
    }

    #[test]
    fn bins_requirement_all_present() {
        // ls is universally available
        assert!(check_bins_requirement(&["ls".to_string()]));
        assert!(!check_bins_requirement(&["nonexistent_binary_xyz_12345".to_string()]));
        assert!(!check_bins_requirement(&["ls".to_string(), "nonexistent_binary_xyz_12345".to_string()]));
    }

    #[test]
    fn any_bins_requirement() {
        assert!(check_any_bins_requirement(&[]));
        assert!(check_any_bins_requirement(&["ls".to_string()]));
        assert!(check_any_bins_requirement(&["nonexistent_binary_xyz".to_string(), "ls".to_string()]));
        assert!(!check_any_bins_requirement(&["nonexistent_binary_xyz".to_string()]));
    }

    #[test]
    fn has_binary_finds_ls() {
        assert!(crate::skills::has_binary("ls"));
        assert!(!crate::skills::has_binary("nonexistent_binary_xyz_12345"));
    }

    #[test]
    fn env_with_config_uses_api_key() {
        // Without api_key, env var must exist
        assert!(!check_env_requirements_with_config(
            &["BEACON_TEST_NONEXISTENT_VAR".to_string()],
            Some("BEACON_TEST_NONEXISTENT_VAR"),
            None,
        ));
        // With api_key matching primary_env, treat as satisfied
        assert!(check_env_requirements_with_config(
            &["BEACON_TEST_NONEXISTENT_VAR".to_string()],
            Some("BEACON_TEST_NONEXISTENT_VAR"),
            Some("sk-test-key"),
        ));
        // Empty api_key doesn't count
        assert!(!check_env_requirements_with_config(
            &["BEACON_TEST_NONEXISTENT_VAR".to_string()],
            Some("BEACON_TEST_NONEXISTENT_VAR"),
            Some(""),
        ));
    }

    #[test]
    fn os_filter_excludes_from_budget_prompt() {
        let mut skill = make_skill("wrong_os", "os gated content", SkillPriority::Standard);
        skill.skill.metadata.os = vec!["nonexistent_os".to_string()];
        let budget = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget);
        assert!(!result.contains("os gated content"));
    }

    #[test]
    fn bins_filter_excludes_from_budget_prompt() {
        let mut skill = make_skill("missing_bin", "bin gated content", SkillPriority::Standard);
        skill.skill.metadata.requires_bins = vec!["nonexistent_binary_xyz_12345".to_string()];
        let budget = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget);
        assert!(!result.contains("bin gated content"));
    }

    #[test]
    fn config_requirement_voice_enabled() {
        assert!(check_config_requirement(&["voice.enabled".to_string()], true));
        assert!(!check_config_requirement(&["voice.enabled".to_string()], false));
    }

    #[test]
    fn config_requirement_unknown_fails_closed() {
        assert!(!check_config_requirement(&["unknown.path".to_string()], true));
    }

    #[test]
    fn config_requirement_empty_passes() {
        assert!(check_config_requirement(&[], false));
    }

    #[test]
    fn config_requirement_filters_from_budget_prompt() {
        let mut skill = make_skill("voice_skill", "voice gated content", SkillPriority::Standard);
        skill.skill.metadata.requires_config = vec!["voice.enabled".to_string()];

        // With voice disabled, skill should be excluded
        let budget_no_voice = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: false,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill.clone()], &budget_no_voice);
        assert!(!result.contains("voice gated content"));

        // With voice enabled, skill should be included
        let budget_voice = PromptBudget {
            max_skills: 50,
            max_chars: 30_000,
            voice_enabled: true,
        };
        let result = build_system_prompt_with_budget("Orin", "", &[skill], &budget_voice);
        assert!(result.contains("voice gated content"));
    }
}
