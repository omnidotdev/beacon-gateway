//! Knowledge selection and injection for persona context

use std::fmt::Write;

use crate::persona::{KnowledgeChunk, KnowledgePriority};

/// Select relevant knowledge chunks based on user message
///
/// Selection priority:
/// 1. All chunks with priority "always"
/// 2. Chunks with tags matching words in the user message
#[must_use]
pub fn select_knowledge<'a>(
    chunks: &'a [KnowledgeChunk],
    user_message: &str,
    max_tokens: usize,
) -> Vec<&'a KnowledgeChunk> {
    let mut selected: Vec<&KnowledgeChunk> = Vec::new();

    // Always-priority chunks first
    for chunk in chunks {
        if chunk.priority == KnowledgePriority::Always {
            selected.push(chunk);
        }
    }

    // Tag-match for relevant chunks
    // Strip punctuation and split into clean tokens
    let message_lower = user_message.to_lowercase();
    let tokens: Vec<String> = message_lower
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|t| !t.is_empty())
        .collect();

    for chunk in chunks {
        if chunk.priority == KnowledgePriority::Relevant {
            let matched = chunk.tags.iter().any(|tag| {
                let tag_lower = tag.to_lowercase();
                tokens.iter().any(|t| *t == tag_lower)
            });
            if matched {
                selected.push(chunk);
            }
        }
    }

    // Trim to token budget
    trim_to_budget(&mut selected, max_tokens);

    selected
}

/// Format selected knowledge chunks for prompt injection
#[must_use]
pub fn format_knowledge(chunks: &[&KnowledgeChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }

    let sections: Vec<String> = chunks
        .iter()
        .map(|chunk| {
            let mut section = format!("## {}\n{}", chunk.topic, chunk.content);
            if !chunk.rules.is_empty() {
                section.push_str("\n\nRules:");
                for rule in &chunk.rules {
                    let _ = write!(section, "\n- {rule}");
                }
            }
            section
        })
        .collect();

    sections.join("\n\n")
}

/// Trim chunks to fit within a token budget (4 chars ≈ 1 token)
fn trim_to_budget(chunks: &mut Vec<&KnowledgeChunk>, max_tokens: usize) {
    let mut total_tokens = 0;
    let mut keep = 0;

    for chunk in chunks.iter() {
        let chunk_tokens = estimate_tokens(&chunk.content) + estimate_tokens(&chunk.topic);
        for rule in &chunk.rules {
            total_tokens += estimate_tokens(rule);
        }
        total_tokens += chunk_tokens;

        if total_tokens > max_tokens && keep > 0 {
            break;
        }
        keep += 1;
    }

    chunks.truncate(keep);
}

/// Rough token estimation (4 chars per token)
const fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::KnowledgePriority;

    fn make_chunk(topic: &str, tags: &[&str], priority: KnowledgePriority) -> KnowledgeChunk {
        KnowledgeChunk {
            topic: topic.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            content: format!("Content about {topic}"),
            rules: vec![],
            priority,
        }
    }

    #[test]
    fn test_always_chunks_included() {
        let chunks = vec![
            make_chunk("Token Info", &["token"], KnowledgePriority::Always),
            make_chunk("Platform", &["platform"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "random question", 10000);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].topic, "Token Info");
    }

    #[test]
    fn test_tag_matching() {
        let chunks = vec![
            make_chunk("Token Info", &["token", "mcg"], KnowledgePriority::Relevant),
            make_chunk("Platform", &["platform", "rigami"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "tell me about the token", 10000);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].topic, "Token Info");
    }

    #[test]
    fn test_multiple_matches() {
        let chunks = vec![
            make_chunk("Token", &["token"], KnowledgePriority::Relevant),
            make_chunk("Platform", &["platform"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "token and platform", 10000);
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn test_always_plus_relevant() {
        let chunks = vec![
            make_chunk("Core", &[], KnowledgePriority::Always),
            make_chunk("Token", &["token"], KnowledgePriority::Relevant),
            make_chunk("Other", &["other"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "what is the token?", 10000);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].topic, "Core");
        assert_eq!(selected[1].topic, "Token");
    }

    #[test]
    fn test_no_matches() {
        let chunks = vec![
            make_chunk("Token", &["token"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "hello world", 10000);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_tag_matching_strips_punctuation() {
        let chunks = vec![
            make_chunk("Token", &["mcg"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "what is $mcg?", 10000);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn test_short_tags_no_false_positives() {
        let chunks = vec![
            make_chunk("AR Platform", &["ar"], KnowledgePriority::Relevant),
        ];

        // "are" should NOT match tag "ar"
        let selected = select_knowledge(&chunks, "what are you?", 10000);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_case_insensitive_matching() {
        let chunks = vec![
            make_chunk("Token", &["MCG"], KnowledgePriority::Relevant),
        ];

        let selected = select_knowledge(&chunks, "what is mcg?", 10000);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn test_token_budget_trimming() {
        let chunks = vec![
            KnowledgeChunk {
                topic: "A".to_string(),
                tags: vec!["a".to_string()],
                content: "Short".to_string(),
                rules: vec![],
                priority: KnowledgePriority::Always,
            },
            KnowledgeChunk {
                topic: "B".to_string(),
                tags: vec!["b".to_string()],
                content: "This is a much longer content string that should push us over the token budget when combined with the first chunk".to_string(),
                rules: vec![],
                priority: KnowledgePriority::Always,
            },
        ];

        // Very tight budget - should keep at least the first chunk
        let selected = select_knowledge(&chunks, "", 5);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].topic, "A");
    }

    #[test]
    fn test_format_knowledge_empty() {
        let formatted = format_knowledge(&[]);
        assert!(formatted.is_empty());
    }

    #[test]
    fn test_format_knowledge_with_rules() {
        let chunk = KnowledgeChunk {
            topic: "Token".to_string(),
            tags: vec![],
            content: "MCG is on Solana".to_string(),
            rules: vec!["Always cite mint address".to_string()],
            priority: KnowledgePriority::Always,
        };

        let formatted = format_knowledge(&[&chunk]);
        assert!(formatted.contains("## Token"));
        assert!(formatted.contains("MCG is on Solana"));
        assert!(formatted.contains("Rules:"));
        assert!(formatted.contains("- Always cite mint address"));
    }
}
