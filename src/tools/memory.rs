//! Built-in memory management tools for the LLM

use std::sync::Arc;

use crate::db::{Embedder, Memory, MemoryCategory, MemoryRepo};
use crate::{Error, Result};

/// Built-in memory tools for LLM direct memory management
pub struct BuiltinMemoryTools {
    memory_repo: MemoryRepo,
    embedder: Option<Arc<Embedder>>,
    user_id: String,
}

impl BuiltinMemoryTools {
    /// Create a new set of built-in memory tools
    #[must_use]
    pub const fn new(memory_repo: MemoryRepo, embedder: Option<Arc<Embedder>>, user_id: String) -> Self {
        Self {
            memory_repo,
            embedder,
            user_id,
        }
    }

    /// Return tool definitions for all built-in memory tools
    #[must_use]
    pub fn tool_definitions() -> Vec<synapse_client::ToolDefinition> {
        vec![
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "memory_store".to_string(),
                    description: Some(
                        "Save important information to long-term memory. Use for user preferences, facts, decisions, and corrections.".to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "The information to remember"
                            },
                            "category": {
                                "type": "string",
                                "enum": ["preference", "fact", "correction", "general"],
                                "description": "Memory category (default: general)"
                            }
                        },
                        "required": ["content"]
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "memory_search".to_string(),
                    description: Some(
                        "Search long-term memory for relevant information. Use to recall past preferences, decisions, or context.".to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Search query"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max results to return (default: 5)"
                            }
                        },
                        "required": ["query"]
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "memory_forget".to_string(),
                    description: Some(
                        "Delete a specific memory by ID. Use when correcting or removing outdated information.".to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Memory ID to delete (from memory_store or memory_search results)"
                            }
                        },
                        "required": ["id"]
                    })),
                },
            },
        ]
    }

    /// Execute a named memory tool
    ///
    /// # Errors
    ///
    /// Returns error if arguments are malformed or database operation fails
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        match name {
            "memory_store" => self.store(arguments).await,
            "memory_search" => self.search(arguments).await,
            "memory_forget" => self.forget(arguments),
            _ => Err(Error::Tool(format!("unknown memory tool: {name}"))),
        }
    }

    async fn store(&self, arguments: &str) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct StoreArgs {
            content: String,
            #[serde(default)]
            category: Option<String>,
        }

        let args: StoreArgs = serde_json::from_str(arguments)
            .map_err(|e| Error::Tool(format!("memory_store: invalid arguments: {e}")))?;

        let category = args
            .category
            .as_deref()
            .and_then(MemoryCategory::from_str_value)
            .unwrap_or(MemoryCategory::General);

        let mut memory = Memory::new(self.user_id.clone(), category, args.content.clone());

        // Embed if embedder available (best-effort: log and continue without)
        if let Some(ref embedder) = self.embedder {
            match embedder.embed(&args.content).await {
                Ok(embedding) => {
                    memory = memory.with_embedding(embedding);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "memory_store: embedding failed, storing without vector");
                }
            }
        }

        self.memory_repo.add(&memory)?;

        let response = serde_json::json!({
            "id": memory.id,
            "status": "stored",
            "content": memory.content
        });

        Ok(response.to_string())
    }

    async fn search(&self, arguments: &str) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct SearchArgs {
            query: String,
            #[serde(default = "default_limit")]
            limit: usize,
        }

        const fn default_limit() -> usize {
            5
        }

        let args: SearchArgs = serde_json::from_str(arguments)
            .map_err(|e| Error::Tool(format!("memory_search: invalid arguments: {e}")))?;

        let memories = if let Some(ref embedder) = self.embedder {
            match embedder.embed(&args.query).await {
                Ok(embedding) => self.memory_repo.search_hybrid(
                    &self.user_id,
                    &args.query,
                    Some(&embedding),
                    args.limit,
                )?,
                Err(e) => {
                    tracing::warn!(error = %e, "memory_search: embedding failed, falling back to text search");
                    self.memory_repo.search(&self.user_id, &args.query)?
                }
            }
        } else {
            self.memory_repo.search(&self.user_id, &args.query)?
        };

        let results: Vec<serde_json::Value> = memories
            .iter()
            .take(args.limit)
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "content": m.content,
                    "category": m.category.to_string(),
                    "tags": m.tags
                })
            })
            .collect();

        Ok(serde_json::json!({ "memories": results }).to_string())
    }

    fn forget(&self, arguments: &str) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct ForgetArgs {
            id: String,
        }

        let args: ForgetArgs = serde_json::from_str(arguments)
            .map_err(|e| Error::Tool(format!("memory_forget: invalid arguments: {e}")))?;

        let deleted = self.memory_repo.delete(&args.id)?;

        let response = if deleted {
            serde_json::json!({ "status": "forgotten", "id": args.id })
        } else {
            serde_json::json!({ "status": "not_found", "id": args.id })
        };

        Ok(response.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tools() -> BuiltinMemoryTools {
        let pool = crate::db::init_memory().unwrap();
        let user_repo = crate::db::UserRepo::new(pool.clone());
        let user = user_repo.find_or_create("test_user").unwrap();
        let repo = MemoryRepo::new(pool);
        BuiltinMemoryTools::new(repo, None, user.id)
    }

    #[tokio::test]
    async fn memory_store_creates_entry() {
        let tools = make_tools();
        let result = tools
            .execute(
                "memory_store",
                r#"{"content":"User prefers dark mode","category":"preference"}"#,
            )
            .await
            .unwrap();
        assert!(
            result.contains("stored") || result.contains("Stored"),
            "result: {result}"
        );
    }

    #[tokio::test]
    async fn memory_search_returns_results() {
        let tools = make_tools();
        // Store something first
        tools
            .execute("memory_store", r#"{"content":"User likes coffee"}"#)
            .await
            .unwrap();
        let result = tools
            .execute("memory_search", r#"{"query":"coffee"}"#)
            .await
            .unwrap();
        assert!(result.contains("coffee"), "result: {result}");
    }

    #[tokio::test]
    async fn memory_forget_deletes_entry() {
        let tools = make_tools();
        // Store and get an ID
        let stored = tools
            .execute("memory_store", r#"{"content":"Temporary memory"}"#)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let id = v["id"].as_str().unwrap().to_string();

        let result = tools
            .execute("memory_forget", &format!(r#"{{"id":"{id}"}}"#))
            .await
            .unwrap();
        assert!(result.contains("forgotten"), "result: {result}");
    }

    #[tokio::test]
    async fn tool_definitions_returns_three_tools() {
        let defs = BuiltinMemoryTools::tool_definitions();
        assert_eq!(defs.len(), 3);
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"memory_store"));
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"memory_forget"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let tools = make_tools();
        let result = tools.execute("memory_unknown", "{}").await;
        assert!(result.is_err());
    }
}
