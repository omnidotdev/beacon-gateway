//! Tool executor — dispatches tool calls to Synapse MCP or builtin handlers

use std::sync::Arc;

use synapse_client::SynapseClient;

use crate::{Error, Result};

/// Executes tool calls via Synapse MCP
pub struct ToolExecutor {
    synapse: Arc<SynapseClient>,
}

impl ToolExecutor {
    /// Create a new tool executor
    pub fn new(synapse: Arc<SynapseClient>) -> Self {
        Self { synapse }
    }

    /// Fetch available MCP tools from Synapse and return as OpenAI tool definitions
    pub async fn list_tools(&self) -> Result<Vec<synapse_client::ToolDefinition>> {
        let mcp_tools = self
            .synapse
            .list_tools(None)
            .await
            .map_err(|e| Error::Tool(e.to_string()))?;

        let definitions = mcp_tools
            .into_iter()
            .map(|t| synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: t.name,
                    description: Some(t.description),
                    parameters: Some(t.input_schema),
                },
            })
            .collect();

        Ok(definitions)
    }

    /// Execute a tool call via Synapse MCP
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).unwrap_or(serde_json::Value::Object(Default::default()));

        let result = self
            .synapse
            .call_tool(name, args)
            .await
            .map_err(|e| Error::Tool(e.to_string()))?;

        // Concatenate text content blocks
        let text = result
            .content
            .iter()
            .filter_map(|block| match block {
                synapse_client::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }
}
