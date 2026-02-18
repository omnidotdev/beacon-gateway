//! life.json export/import for memory portability
//!
//! Export converts memories into the life.json `learnedFacts` format.
//! Import parses `learnedFacts` from a life.json and creates memories,
//! deduplicating by content hash

use std::collections::HashMap;

use crate::db::{Memory, MemoryCategory, MemoryRepo};
use crate::Result;

use super::life_json::{AssistantConfig, LearnedFact, LifeJson};

/// Maximum number of memories to include in an export
const DEFAULT_EXPORT_LIMIT: usize = 50;

/// Source channel tag for memories imported from life.json
const LIFE_JSON_SOURCE: &str = "life.json";

/// Export result with counts
#[derive(Debug)]
pub struct ExportResult {
    /// The life.json assistants section containing exported memories
    pub life_json: LifeJson,
    /// Number of memories exported
    pub count: usize,
}

/// Import result with counts
#[derive(Debug)]
pub struct ImportResult {
    /// Number of memories successfully imported
    pub imported: usize,
    /// Number of memories skipped (duplicates)
    pub skipped: usize,
}

/// Export memories to life.json `learnedFacts` format
///
/// Selects pinned memories and top-accessed memories (up to `limit`) and
/// formats them as `assistants.<persona_id>.learnedFacts` entries.
///
/// # Errors
///
/// Returns error if database query fails
pub fn export_memories(
    repo: &MemoryRepo,
    user_id: &str,
    persona_id: &str,
    limit: Option<usize>,
) -> Result<ExportResult> {
    let max = limit.unwrap_or(DEFAULT_EXPORT_LIMIT);
    let memories = repo.get_exportable(user_id, max)?;

    let facts: Vec<LearnedFact> = memories
        .iter()
        .map(|m| LearnedFact {
            fact: m.content.clone(),
            confidence: Some(confidence_from_memory(m)),
            source: Some(source_label(m)),
        })
        .collect();

    let count = facts.len();

    let config = AssistantConfig {
        learned_facts: if facts.is_empty() { None } else { Some(facts) },
        ..AssistantConfig::default()
    };

    let mut assistants = HashMap::new();
    assistants.insert(persona_id.to_string(), config);

    let life_json = LifeJson {
        version: Some("1.0.0".to_string()),
        assistants: Some(assistants),
        ..LifeJson::default()
    };

    tracing::info!(user_id, persona_id, count, "exported memories to life.json format");

    Ok(ExportResult { life_json, count })
}

/// Import memories from life.json content
///
/// Parses `learnedFacts` from the specified assistant section (or all
/// assistants if `persona_id` is `None`) and creates memories, skipping
/// any whose content hash already exists for the user.
///
/// # Errors
///
/// Returns error if JSON parsing or database operations fail
pub fn import_memories(
    repo: &MemoryRepo,
    user_id: &str,
    content: &str,
    persona_id: Option<&str>,
) -> Result<ImportResult> {
    let life_json: LifeJson = serde_json::from_str(content)?;

    let mut imported = 0;
    let mut skipped = 0;

    let assistants = match life_json.assistants {
        Some(a) => a,
        None => return Ok(ImportResult { imported: 0, skipped: 0 }),
    };

    for (assistant_id, config) in &assistants {
        // If a specific persona was requested, skip others
        if let Some(pid) = persona_id {
            if assistant_id != pid {
                continue;
            }
        }

        let facts = match &config.learned_facts {
            Some(f) => f,
            None => continue,
        };

        for fact in facts {
            let content_hash = Memory::compute_content_hash(&fact.fact);

            // Skip duplicates by content hash
            if repo.exists_by_content_hash(user_id, &content_hash)? {
                skipped += 1;
                continue;
            }

            let category = category_from_source(fact.source.as_deref());
            let memory = Memory::new(user_id.to_string(), category, fact.fact.clone())
                .with_source(String::new(), LIFE_JSON_SOURCE.to_string());

            repo.add(&memory)?;
            imported += 1;
        }
    }

    tracing::info!(user_id, imported, skipped, "imported memories from life.json");

    Ok(ImportResult { imported, skipped })
}

/// Derive a confidence score from memory metadata
///
/// Pinned memories get high confidence, then scaled by access count
fn confidence_from_memory(memory: &Memory) -> f32 {
    if memory.pinned {
        return 1.0;
    }

    // Scale access count to 0.5..0.95 range
    let base = 0.5_f32;
    let scale = (memory.access_count as f32 / 100.0).min(0.45);
    base + scale
}

/// Build a human-readable source label for the exported fact
fn source_label(memory: &Memory) -> String {
    match &memory.source_channel {
        Some(ch) if !ch.is_empty() => format!("beacon:{}", ch),
        _ => "beacon".to_string(),
    }
}

/// Map a life.json fact source hint to a memory category
fn category_from_source(source: Option<&str>) -> MemoryCategory {
    match source {
        Some(s) if s.contains("preference") => MemoryCategory::Preference,
        Some(s) if s.contains("correction") => MemoryCategory::Correction,
        _ => MemoryCategory::Fact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup() -> (MemoryRepo, String) {
        let pool = db::init_memory().unwrap();
        let repo = MemoryRepo::new(pool.clone());
        let user_repo = crate::db::UserRepo::new(pool);
        let user = user_repo.find_or_create("life_json_user").unwrap();
        (repo, user.id)
    }

    #[test]
    fn test_export_empty() {
        let (repo, user_id) = setup();
        let result = export_memories(&repo, &user_id, "orin", None).unwrap();
        assert_eq!(result.count, 0);
    }

    #[test]
    fn test_export_with_memories() {
        let (repo, user_id) = setup();

        let m1 = Memory::new(user_id.clone(), MemoryCategory::Fact, "Likes Rust".to_string()).pinned();
        let m2 = Memory::new(user_id.clone(), MemoryCategory::Preference, "Dark mode".to_string());
        repo.add(&m1).unwrap();
        repo.add(&m2).unwrap();

        let result = export_memories(&repo, &user_id, "orin", None).unwrap();
        assert_eq!(result.count, 2);

        let assistants = result.life_json.assistants.unwrap();
        let config = assistants.get("orin").unwrap();
        let facts = config.learned_facts.as_ref().unwrap();
        assert_eq!(facts.len(), 2);

        // Pinned memory should have confidence 1.0
        let pinned_fact = facts.iter().find(|f| f.fact == "Likes Rust").unwrap();
        assert_eq!(pinned_fact.confidence, Some(1.0));
    }

    #[test]
    fn test_import_basic() {
        let (repo, user_id) = setup();

        let json = r#"{
            "version": "1.0.0",
            "assistants": {
                "orin": {
                    "learnedFacts": [
                        {"fact": "Prefers Rust", "confidence": 0.9, "source": "beacon"},
                        {"fact": "Lives in Portland", "confidence": 0.8}
                    ]
                }
            }
        }"#;

        let result = import_memories(&repo, &user_id, json, None).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.skipped, 0);

        // Verify memories were created
        let memories = repo.list(&user_id, None).unwrap();
        assert_eq!(memories.len(), 2);
    }

    #[test]
    fn test_import_dedup() {
        let (repo, user_id) = setup();

        // Pre-create a memory with same content
        let existing = Memory::new(user_id.clone(), MemoryCategory::Fact, "Prefers Rust".to_string());
        repo.add(&existing).unwrap();

        let json = r#"{
            "version": "1.0.0",
            "assistants": {
                "orin": {
                    "learnedFacts": [
                        {"fact": "Prefers Rust"},
                        {"fact": "New fact"}
                    ]
                }
            }
        }"#;

        let result = import_memories(&repo, &user_id, json, None).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_import_filtered_persona() {
        let (repo, user_id) = setup();

        let json = r#"{
            "version": "1.0.0",
            "assistants": {
                "orin": {
                    "learnedFacts": [{"fact": "For Orin"}]
                },
                "other": {
                    "learnedFacts": [{"fact": "For Other"}]
                }
            }
        }"#;

        let result = import_memories(&repo, &user_id, json, Some("orin")).unwrap();
        assert_eq!(result.imported, 1);

        let memories = repo.list(&user_id, None).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].content, "For Orin");
    }

    #[test]
    fn test_import_source_channel() {
        let (repo, user_id) = setup();

        let json = r#"{
            "version": "1.0.0",
            "assistants": {
                "orin": {
                    "learnedFacts": [{"fact": "Test fact"}]
                }
            }
        }"#;

        import_memories(&repo, &user_id, json, None).unwrap();

        let memories = repo.list(&user_id, None).unwrap();
        assert_eq!(memories[0].source_channel.as_deref(), Some("life.json"));
    }

    #[test]
    fn test_roundtrip() {
        let (repo, user_id) = setup();

        // Create memories
        let m1 = Memory::new(user_id.clone(), MemoryCategory::Fact, "Knows Rust".to_string()).pinned();
        let m2 = Memory::new(user_id.clone(), MemoryCategory::Preference, "Vim user".to_string());
        repo.add(&m1).unwrap();
        repo.add(&m2).unwrap();

        // Export
        let export = export_memories(&repo, &user_id, "orin", None).unwrap();
        let json = serde_json::to_string(&export.life_json).unwrap();

        // Import into a fresh database
        let pool2 = db::init_memory().unwrap();
        let repo2 = MemoryRepo::new(pool2.clone());
        let user_repo2 = crate::db::UserRepo::new(pool2);
        let user2 = user_repo2.find_or_create("roundtrip_user").unwrap();

        let import = import_memories(&repo2, &user2.id, &json, Some("orin")).unwrap();
        assert_eq!(import.imported, 2);

        let memories = repo2.list(&user2.id, None).unwrap();
        assert_eq!(memories.len(), 2);
    }

    #[test]
    fn test_confidence_from_memory() {
        let pinned = Memory::new("u".to_string(), MemoryCategory::Fact, "test".to_string()).pinned();
        assert_eq!(confidence_from_memory(&pinned), 1.0);

        let basic = Memory::new("u".to_string(), MemoryCategory::Fact, "test".to_string());
        assert!((confidence_from_memory(&basic) - 0.5).abs() < f32::EPSILON);
    }
}
