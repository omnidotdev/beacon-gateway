//! Context builder for assembling conversation context

use crate::db::{Memory, MemoryRepo, Message, MessageRole, SessionRepo, UserContext, UserRepo};
use crate::Result;

use super::life_json::{LifeJson, LifeJsonReader};

/// Configuration for context building
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Maximum number of messages to include from history
    pub max_messages: usize,
    /// Maximum approximate token count for context
    pub max_tokens: usize,
    /// Persona/assistant ID for life.json lookup
    pub persona_id: String,
    /// Maximum number of memories to include
    pub max_memories: usize,
    /// Persona system prompt to include in context
    pub persona_system_prompt: Option<String>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_messages: 20,
            max_tokens: 4000,
            persona_id: "orin".to_string(),
            max_memories: 10,
            persona_system_prompt: None,
        }
    }
}

/// Built context ready for injection into agent
#[derive(Debug, Clone)]
pub struct BuiltContext {
    /// Persona system prompt (personality, instructions)
    pub persona_prompt: Option<String>,
    /// Selected knowledge for this turn
    pub knowledge_context: String,
    /// System context (life.json + user context)
    pub system_context: String,
    /// Recent messages for conversation history
    pub messages: Vec<ContextMessage>,
    /// Approximate token count
    pub estimated_tokens: usize,
}

impl BuiltContext {
    /// Format the full prompt including persona, user context and conversation history
    ///
    /// Returns a prompt string that includes:
    /// - Persona system prompt (personality, instructions)
    /// - User context from life.json and learned preferences
    /// - Recent conversation history
    /// - The current user message
    #[must_use]
    pub fn format_prompt(&self, current_message: &str) -> String {
        let mut parts = Vec::new();

        // Add persona system prompt if present
        if let Some(persona_prompt) = &self.persona_prompt {
            if !persona_prompt.is_empty() {
                parts.push(format!("<persona>\n{persona_prompt}\n</persona>"));
            }
        }

        // Add knowledge context if present
        if !self.knowledge_context.is_empty() {
            parts.push(format!(
                "<knowledge>\n{}\n</knowledge>",
                self.knowledge_context
            ));
        }

        // Add user context if present
        if !self.system_context.is_empty() {
            parts.push(format!("<user-context>\n{}\n</user-context>", self.system_context));
        }

        // Add conversation history if present
        if !self.messages.is_empty() {
            let history: Vec<String> = self
                .messages
                .iter()
                .map(|m| format!("<{}>\n{}\n</{}>", m.role, m.content, m.role))
                .collect();
            parts.push(format!(
                "<conversation-history>\n{}\n</conversation-history>",
                history.join("\n")
            ));
        }

        // Add current message
        parts.push(current_message.to_string());

        parts.join("\n\n")
    }
}

/// A message in the context
#[derive(Debug, Clone)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

/// Builds context for AI conversations
pub struct ContextBuilder {
    config: ContextConfig,
}

impl ContextBuilder {
    /// Create a new context builder
    #[must_use]
    pub const fn new(config: ContextConfig) -> Self {
        Self { config }
    }

    /// Build context for a session
    ///
    /// # Errors
    ///
    /// Returns error if database operations fail
    pub fn build(
        &self,
        session_id: &str,
        user_id: &str,
        life_json_path: Option<&str>,
        session_repo: &SessionRepo,
        user_repo: &UserRepo,
    ) -> Result<BuiltContext> {
        self.build_with_thread(
            session_id,
            user_id,
            life_json_path,
            session_repo,
            user_repo,
            None,
            None,
        )
    }

    /// Build context for a session with memory support
    ///
    /// # Errors
    ///
    /// Returns error if database operations fail
    pub fn build_with_memory(
        &self,
        session_id: &str,
        user_id: &str,
        life_json_path: Option<&str>,
        session_repo: &SessionRepo,
        user_repo: &UserRepo,
        memory_repo: Option<&MemoryRepo>,
    ) -> Result<BuiltContext> {
        self.build_with_thread(
            session_id,
            user_id,
            life_json_path,
            session_repo,
            user_repo,
            memory_repo,
            None,
        )
    }

    /// Build context for a session with memory and thread support
    ///
    /// When `thread_id` is provided, only messages from that thread are included
    /// in the conversation history.
    ///
    /// # Errors
    ///
    /// Returns error if database operations fail
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_thread(
        &self,
        session_id: &str,
        user_id: &str,
        life_json_path: Option<&str>,
        session_repo: &SessionRepo,
        user_repo: &UserRepo,
        memory_repo: Option<&MemoryRepo>,
        thread_id: Option<&str>,
    ) -> Result<BuiltContext> {
        let mut system_parts = Vec::new();

        // Load life.json context
        if let Some(path) = life_json_path {
            if let Ok(life_json) = LifeJsonReader::read(path) {
                let life_context = life_json.build_context_string(&self.config.persona_id);
                if !life_context.is_empty() {
                    system_parts.push(life_context);
                }
            }
        }

        // Load memories from database
        if let Some(repo) = memory_repo {
            let memories = repo.get_context(user_id, self.config.max_memories).unwrap_or_default();
            if !memories.is_empty() {
                let memory_context = format_memories(&memories);
                if !memory_context.is_empty() {
                    system_parts.push(memory_context);
                }
            }
        }

        // Load learned user context from database
        let user_contexts = user_repo.get_context(user_id).unwrap_or_default();
        if !user_contexts.is_empty() {
            let learned_context = format_user_context(&user_contexts);
            if !learned_context.is_empty() {
                system_parts.push(learned_context);
            }
        }

        let system_context = system_parts.join("\n\n");

        // Load recent messages (filtered by thread if specified)
        let messages = if thread_id.is_some() {
            session_repo
                .get_messages_in_thread(session_id, thread_id, self.config.max_messages)
                .unwrap_or_default()
        } else {
            session_repo
                .get_messages(session_id, self.config.max_messages)
                .unwrap_or_default()
        };

        // Convert to context messages and apply pruning
        let (context_messages, estimated_tokens) =
            self.prune_messages(&messages, &system_context);

        Ok(BuiltContext {
            persona_prompt: self.config.persona_system_prompt.clone(),
            knowledge_context: String::new(),
            system_context,
            messages: context_messages,
            estimated_tokens,
        })
    }

    /// Build context from just a life.json file (for initial setup)
    #[must_use]
    pub fn build_from_life_json(&self, life_json: &LifeJson) -> BuiltContext {
        let system_context = life_json.build_context_string(&self.config.persona_id);
        let estimated_tokens = estimate_tokens(&system_context);

        BuiltContext {
            persona_prompt: self.config.persona_system_prompt.clone(),
            knowledge_context: String::new(),
            system_context,
            messages: Vec::new(),
            estimated_tokens,
        }
    }

    /// Prune messages to fit within token budget
    fn prune_messages(
        &self,
        messages: &[Message],
        system_context: &str,
    ) -> (Vec<ContextMessage>, usize) {
        let system_tokens = estimate_tokens(system_context);
        let _available_tokens = self.config.max_tokens.saturating_sub(system_tokens);

        let mut context_messages = Vec::new();
        let mut used_tokens = system_tokens;

        // Process messages from oldest to newest (they come in chronological order)
        for msg in messages {
            let msg_tokens = estimate_tokens(&msg.content);

            // Check if we'd exceed the budget
            if used_tokens + msg_tokens > self.config.max_tokens {
                // If we have no messages yet, include at least the last one
                if context_messages.is_empty() {
                    context_messages.push(ContextMessage {
                        role: role_to_string(msg.role),
                        content: msg.content.clone(),
                    });
                    used_tokens += msg_tokens;
                }
                break;
            }

            context_messages.push(ContextMessage {
                role: role_to_string(msg.role),
                content: msg.content.clone(),
            });
            used_tokens += msg_tokens;
        }

        (context_messages, used_tokens)
    }
}

/// Format memories for system prompt
fn format_memories(memories: &[Memory]) -> String {
    if memories.is_empty() {
        return String::new();
    }

    let entries: Vec<String> = memories
        .iter()
        .map(|m| format!("- [{}] {}", m.category, m.content))
        .collect();

    format!("Remembered facts about you:\n{}", entries.join("\n"))
}

/// Format user context entries for system prompt
fn format_user_context(contexts: &[UserContext]) -> String {
    if contexts.is_empty() {
        return String::new();
    }

    let entries: Vec<String> = contexts
        .iter()
        .map(|ctx| format!("{}: {}", ctx.key, ctx.value))
        .collect();

    format!("Learned user preferences:\n{}", entries.join("\n"))
}

/// Convert message role to string
fn role_to_string(role: MessageRole) -> String {
    match role {
        MessageRole::User => "user".to_string(),
        MessageRole::Assistant => "assistant".to_string(),
        MessageRole::System => "system".to_string(),
    }
}

/// Rough token estimation (4 chars per token on average)
const fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 1);
        assert_eq!(estimate_tokens("hello world"), 2);
    }

    #[test]
    fn test_format_user_context() {
        let contexts = vec![
            UserContext {
                id: "1".to_string(),
                user_id: "u1".to_string(),
                key: "timezone".to_string(),
                value: "America/Los_Angeles".to_string(),
                source: "learned".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];

        let formatted = format_user_context(&contexts);
        assert!(formatted.contains("timezone: America/Los_Angeles"));
    }

    #[test]
    fn test_context_config_default() {
        let config = ContextConfig::default();
        assert_eq!(config.max_messages, 20);
        assert_eq!(config.max_tokens, 4000);
    }

    #[test]
    fn test_format_prompt_with_context_and_history() {
        let ctx = BuiltContext {
            persona_prompt: None,
            knowledge_context: String::new(),
            system_context: "User's name: Brian\nTimezone: America/Los_Angeles".to_string(),
            messages: vec![
                ContextMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                },
                ContextMessage {
                    role: "assistant".to_string(),
                    content: "Hi there!".to_string(),
                },
            ],
            estimated_tokens: 100,
        };

        let prompt = ctx.format_prompt("What time is it?");

        assert!(prompt.contains("<user-context>"));
        assert!(prompt.contains("User's name: Brian"));
        assert!(prompt.contains("<conversation-history>"));
        assert!(prompt.contains("<user>\nHello\n</user>"));
        assert!(prompt.contains("<assistant>\nHi there!\n</assistant>"));
        assert!(prompt.contains("What time is it?"));
    }

    #[test]
    fn test_format_prompt_empty_context() {
        let ctx = BuiltContext {
            persona_prompt: None,
            knowledge_context: String::new(),
            system_context: String::new(),
            messages: Vec::new(),
            estimated_tokens: 0,
        };

        let prompt = ctx.format_prompt("Hello");
        assert_eq!(prompt, "Hello");
    }

    #[test]
    fn test_format_prompt_with_knowledge() {
        let ctx = BuiltContext {
            persona_prompt: Some("You are MC".to_string()),
            knowledge_context: "## Token\nMCG on Solana".to_string(),
            system_context: String::new(),
            messages: Vec::new(),
            estimated_tokens: 50,
        };

        let prompt = ctx.format_prompt("what is mcg?");
        assert!(prompt.contains("<persona>"));
        assert!(prompt.contains("<knowledge>"));
        assert!(prompt.contains("## Token"));
        // Knowledge should come after persona, before user-context
        let persona_pos = prompt.find("<persona>").unwrap();
        let knowledge_pos = prompt.find("<knowledge>").unwrap();
        assert!(knowledge_pos > persona_pos);
    }
}
