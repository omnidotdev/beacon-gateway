//! Conversation indexer for extracting and storing memories
//!
//! Extracts facts, preferences, and corrections from conversations and stores
//! them as memories with embeddings for semantic retrieval

use serde::{Deserialize, Serialize};

use super::embedder::Embedder;
use super::memory::{Memory, MemoryCategory, MemoryRepo};
use crate::{Error, Result};

/// Extracted fact from a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    /// The fact content
    pub content: String,
    /// Category of the fact
    pub category: String,
    /// Relevance tags
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response from fact extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResponse {
    /// List of extracted facts
    pub facts: Vec<ExtractedFact>,
}

/// Conversation indexer
pub struct Indexer {
    embedder: Embedder,
    memory_repo: MemoryRepo,
    client: reqwest::Client,
    api_key: String,
}

impl Indexer {
    /// Create a new indexer
    ///
    /// # Errors
    ///
    /// Returns error if embedder cannot be created
    #[must_use]
    pub fn new(embedder: Embedder, memory_repo: MemoryRepo, openai_api_key: String) -> Self {
        Self {
            embedder,
            memory_repo,
            client: reqwest::Client::new(),
            api_key: openai_api_key,
        }
    }

    /// Extract facts from a conversation and store as memories
    ///
    /// # Arguments
    ///
    /// * `user_id` - User ID to associate memories with
    /// * `conversation` - The conversation text to analyze
    /// * `session_id` - Optional session ID for source tracking
    /// * `channel` - Optional channel name for source tracking
    ///
    /// # Errors
    ///
    /// Returns error if extraction or storage fails
    pub async fn index_conversation(
        &self,
        user_id: &str,
        conversation: &str,
        session_id: Option<&str>,
        channel: Option<&str>,
    ) -> Result<Vec<Memory>> {
        // Extract facts using LLM
        let extracted = self.extract_facts(conversation).await?;

        if extracted.facts.is_empty() {
            return Ok(Vec::new());
        }

        // Prepare texts for batch embedding
        let contents: Vec<&str> = extracted.facts.iter().map(|f| f.content.as_str()).collect();
        let embeddings = self.embedder.embed_batch(&contents).await?;

        // Create and store memories
        let mut memories = Vec::new();

        for (fact, embedding) in extracted.facts.into_iter().zip(embeddings.into_iter()) {
            let category = match fact.category.as_str() {
                "preference" => MemoryCategory::Preference,
                "correction" => MemoryCategory::Correction,
                "fact" => MemoryCategory::Fact,
                _ => MemoryCategory::General,
            };

            let mut memory = Memory::new(user_id.to_string(), category, fact.content)
                .with_embedding(embedding);

            // Add tags
            for tag in fact.tags {
                memory = memory.with_tag(tag);
            }

            // Add source info if provided
            if let (Some(sid), Some(ch)) = (session_id, channel) {
                memory = memory.with_source(sid.to_string(), ch.to_string());
            }

            self.memory_repo.add(&memory)?;
            memories.push(memory);
        }

        tracing::info!(
            user_id,
            count = memories.len(),
            "indexed conversation facts"
        );

        Ok(memories)
    }

    /// Extract facts from text using LLM
    #[allow(clippy::items_after_statements)]
    async fn extract_facts(&self, conversation: &str) -> Result<ExtractionResponse> {
        let system_prompt = r#"You extract facts, preferences, and corrections from conversations.

Output JSON with this structure:
{
  "facts": [
    {"content": "...", "category": "preference|fact|correction", "tags": ["tag1"]}
  ]
}

Categories:
- preference: How the user likes things done (communication style, preferences)
- fact: Factual information about the user (name, location, job, relationships)
- correction: When the user corrects a previous assumption or error

Only extract meaningful, persistent information. Ignore:
- Temporary states ("I'm tired")
- One-time requests ("Tell me a joke")
- Greetings and pleasantries

Be concise. Each fact should be a single, clear statement."#;

        let user_prompt = format!("Extract facts from this conversation:\n\n{conversation}");

        #[derive(Serialize)]
        struct ChatRequest {
            model: String,
            messages: Vec<ChatMessage>,
            response_format: ResponseFormat,
        }

        #[derive(Serialize)]
        struct ChatMessage {
            role: String,
            content: String,
        }

        #[derive(Serialize)]
        struct ResponseFormat {
            #[serde(rename = "type")]
            format_type: String,
        }

        let request = ChatRequest {
            model: "gpt-4o-mini".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ],
            response_format: ResponseFormat {
                format_type: "json_object".to_string(),
            },
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Embedding(format!(
                "Fact extraction API error {status}: {body}"
            )));
        }

        #[derive(Deserialize)]
        struct ChatResponse {
            choices: Vec<Choice>,
        }

        #[derive(Deserialize)]
        struct Choice {
            message: ChoiceMessage,
        }

        #[derive(Deserialize)]
        struct ChoiceMessage {
            content: String,
        }

        let chat_response: ChatResponse = response.json().await?;

        let content = chat_response
            .choices
            .first()
            .map_or("{}", |c| c.message.content.as_str());

        let extraction: ExtractionResponse =
            serde_json::from_str(content).unwrap_or(ExtractionResponse { facts: Vec::new() });

        Ok(extraction)
    }

    /// Index a single user message (for incremental indexing)
    ///
    /// # Errors
    ///
    /// Returns error if extraction or storage fails
    pub async fn index_message(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_response: &str,
        session_id: Option<&str>,
        channel: Option<&str>,
    ) -> Result<Vec<Memory>> {
        let conversation = format!("User: {user_message}\nAssistant: {assistant_response}");

        self.index_conversation(user_id, &conversation, session_id, channel)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extraction_response_parse() {
        let json = r#"{
            "facts": [
                {"content": "User prefers dark mode", "category": "preference", "tags": ["ui"]},
                {"content": "User works at Acme Corp", "category": "fact", "tags": ["work"]}
            ]
        }"#;

        let response: ExtractionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.facts.len(), 2);
        assert_eq!(response.facts[0].category, "preference");
        assert_eq!(response.facts[1].tags, vec!["work"]);
    }

    #[test]
    fn test_empty_response() {
        let json = r#"{"facts": []}"#;
        let response: ExtractionResponse = serde_json::from_str(json).unwrap();
        assert!(response.facts.is_empty());
    }
}
