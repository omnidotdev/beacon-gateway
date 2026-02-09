//! life.json reader for portable digital identity

use std::path::Path;

use serde::Deserialize;

use crate::{Error, Result};

/// life.json root structure (partial - only what Beacon needs)
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LifeJson {
    pub version: Option<String>,
    pub identity: Option<Identity>,
    pub preferences: Option<Preferences>,
    pub assistants: Option<std::collections::HashMap<String, AssistantConfig>>,
    pub calendar: Option<Calendar>,
}

/// Identity slice
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub name: Option<String>,
    pub full_name: Option<String>,
    pub nickname: Option<String>,
    pub pronouns: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub bio: Option<String>,
    pub occupation: Option<String>,
    pub location: Option<Location>,
}

/// Location within identity
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Location {
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
}

/// Preferences slice
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub language: Option<String>,
    pub theme: Option<String>,
    pub communication: Option<Communication>,
    pub units: Option<Units>,
}

/// Communication preferences
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Communication {
    pub style: Option<String>,
    pub channels: Option<Vec<String>>,
}

/// Unit preferences
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Units {
    pub temperature: Option<String>,
    pub distance: Option<String>,
    pub date_format: Option<String>,
    pub time_format: Option<String>,
}

/// Per-assistant configuration from life.json
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantConfig {
    pub enabled: Option<bool>,
    pub learned_facts: Option<Vec<LearnedFact>>,
    pub preferences: Option<AssistantPreferences>,
    pub context: Option<AssistantContext>,
    pub permissions: Option<AssistantPermissions>,
}

/// A learned fact about the user
#[derive(Debug, Clone, Deserialize)]
pub struct LearnedFact {
    pub fact: String,
    pub confidence: Option<f32>,
    pub source: Option<String>,
}

/// Per-assistant interaction preferences
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantPreferences {
    pub verbosity: Option<String>,
    pub tone: Option<String>,
    pub expertise: Option<Vec<String>>,
    pub avoid_topics: Option<Vec<String>>,
}

/// Per-assistant context
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantContext {
    pub current_projects: Option<Vec<String>>,
    pub goals: Option<Vec<String>>,
    pub notes: Option<String>,
}

/// Per-assistant permissions
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct AssistantPermissions {
    pub can_learn: Option<bool>,
    pub can_access_calendar: Option<bool>,
    pub can_access_contacts: Option<bool>,
    pub can_access_files: Option<bool>,
    pub can_execute_code: Option<bool>,
    pub can_browse_web: Option<bool>,
}

/// Calendar slice (scheduling preferences)
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Calendar {
    pub scheduling: Option<Scheduling>,
}

/// Scheduling preferences
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scheduling {
    pub working_hours: Option<WorkingHours>,
    pub booking_url: Option<String>,
}

/// Working hours configuration
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkingHours {
    pub start: Option<String>,
    pub end: Option<String>,
    pub days: Option<Vec<String>>,
}

/// Reader for life.json files
pub struct LifeJsonReader;

impl LifeJsonReader {
    /// Read and parse a life.json file
    ///
    /// # Errors
    ///
    /// Returns error if file cannot be read or parsed
    pub fn read<P: AsRef<Path>>(path: P) -> Result<LifeJson> {
        let path = path.as_ref();

        if !path.exists() {
            return Ok(LifeJson::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("failed to read life.json: {e}")))?;

        let life_json: LifeJson = serde_json::from_str(&content)
            .map_err(|e| Error::Config(format!("failed to parse life.json: {e}")))?;

        tracing::debug!(path = %path.display(), "loaded life.json");
        Ok(life_json)
    }

    /// Get assistant-specific config from life.json
    #[must_use]
    pub fn get_assistant_config<'a>(life_json: &'a LifeJson, assistant_id: &str) -> Option<&'a AssistantConfig> {
        life_json.assistants.as_ref()?.get(assistant_id)
    }
}

impl LifeJson {
    /// Build a context string from identity and preferences for the assistant
    #[must_use]
    pub fn build_context_string(&self, assistant_id: &str) -> String {
        let mut parts = Vec::new();

        // Identity context
        if let Some(identity) = &self.identity {
            if let Some(name) = &identity.name {
                parts.push(format!("User's name: {name}"));
            }
            if let Some(pronouns) = &identity.pronouns {
                parts.push(format!("Pronouns: {pronouns}"));
            }
            if let Some(timezone) = &identity.timezone {
                parts.push(format!("Timezone: {timezone}"));
            }
            if let Some(occupation) = &identity.occupation {
                parts.push(format!("Occupation: {occupation}"));
            }
            if let Some(location) = &identity.location {
                let loc_parts: Vec<&str> = [
                    location.city.as_deref(),
                    location.region.as_deref(),
                    location.country.as_deref(),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !loc_parts.is_empty() {
                    parts.push(format!("Location: {}", loc_parts.join(", ")));
                }
            }
        }

        // Preferences context
        if let Some(prefs) = &self.preferences {
            if let Some(comm) = &prefs.communication {
                if let Some(style) = &comm.style {
                    parts.push(format!("Communication style preference: {style}"));
                }
            }
            if let Some(units) = &prefs.units {
                if let Some(temp) = &units.temperature {
                    parts.push(format!("Temperature unit: {temp}"));
                }
                if let Some(time) = &units.time_format {
                    parts.push(format!("Time format: {time}"));
                }
            }
        }

        // Assistant-specific context
        if let Some(assistants) = &self.assistants {
            if let Some(config) = assistants.get(assistant_id) {
                // Learned facts
                if let Some(facts) = &config.learned_facts {
                    for fact in facts.iter().take(10) {
                        parts.push(format!("Known fact: {}", fact.fact));
                    }
                }

                // Expertise areas
                if let Some(prefs) = &config.preferences {
                    if let Some(expertise) = &prefs.expertise {
                        if !expertise.is_empty() {
                            parts.push(format!(
                                "User has expertise in: {}",
                                expertise.join(", ")
                            ));
                        }
                    }
                    if let Some(avoid) = &prefs.avoid_topics {
                        if !avoid.is_empty() {
                            parts.push(format!(
                                "Topics to avoid: {}",
                                avoid.join(", ")
                            ));
                        }
                    }
                }

                // Current context
                if let Some(ctx) = &config.context {
                    if let Some(projects) = &ctx.current_projects {
                        if !projects.is_empty() {
                            parts.push(format!(
                                "Current projects: {}",
                                projects.join(", ")
                            ));
                        }
                    }
                    if let Some(goals) = &ctx.goals {
                        if !goals.is_empty() {
                            parts.push(format!("Current goals: {}", goals.join(", ")));
                        }
                    }
                }
            }
        }

        if parts.is_empty() {
            String::new()
        } else {
            format!("User context from life.json:\n{}", parts.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal() {
        let json = r#"{"version": "1.0.0"}"#;
        let life_json: LifeJson = serde_json::from_str(json).unwrap();
        assert_eq!(life_json.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_with_identity() {
        let json = r#"{
            "version": "1.0.0",
            "identity": {
                "name": "Brian",
                "timezone": "America/Los_Angeles"
            }
        }"#;
        let life_json: LifeJson = serde_json::from_str(json).unwrap();
        assert_eq!(life_json.identity.as_ref().unwrap().name, Some("Brian".to_string()));
    }

    #[test]
    fn test_build_context_string() {
        let json = r#"{
            "version": "1.0.0",
            "identity": {
                "name": "Brian",
                "timezone": "America/Los_Angeles",
                "occupation": "Software Engineer"
            },
            "assistants": {
                "orin": {
                    "learnedFacts": [
                        {"fact": "Prefers Rust"}
                    ],
                    "preferences": {
                        "expertise": ["rust", "typescript"]
                    }
                }
            }
        }"#;
        let life_json: LifeJson = serde_json::from_str(json).unwrap();
        let context = life_json.build_context_string("orin");

        assert!(context.contains("Brian"));
        assert!(context.contains("America/Los_Angeles"));
        assert!(context.contains("Prefers Rust"));
        assert!(context.contains("rust, typescript"));
    }

    #[test]
    fn test_read_nonexistent_returns_default() {
        let result = LifeJsonReader::read("/nonexistent/path/life.json");
        assert!(result.is_ok());
    }
}
