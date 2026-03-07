//! MCP protocol types

use serde::{Deserialize, Serialize};

/// Configuration for a single MCP server
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    /// Display name for the server
    pub name: String,
    /// Command to execute (e.g. "npx", "uvx", "node")
    pub command: String,
    /// Arguments to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// An MCP tool definition received from a server
#[derive(Debug, Clone)]
pub struct McpTool {
    /// Tool name as declared by the server
    pub name: String,
    /// Human-readable description
    pub description: Option<String>,
    /// JSON Schema for the tool's input
    pub input_schema: serde_json::Value,
    /// Which server owns this tool
    pub server_name: String,
}

/// Result from an MCP tool call
#[derive(Debug, Clone)]
pub struct McpToolResult {
    /// Text content from the tool response
    pub text: String,
    /// Whether the tool reported an error
    pub is_error: bool,
}

// --- JSON-RPC types for the MCP protocol ---

#[derive(Debug, Serialize)]
pub(super) struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// MCP `initialize` result
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(super) struct InitializeResult {
    pub protocol_version: String,
    pub server_info: Option<ServerInfo>,
    pub capabilities: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(super) struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

/// MCP `tools/list` result
#[derive(Debug, Deserialize)]
pub(super) struct ToolsListResult {
    pub tools: Vec<McpToolDef>,
}

/// A single tool from `tools/list`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// MCP `tools/call` result
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ToolCallResult {
    pub content: Vec<ToolContent>,
    #[serde(default)]
    pub is_error: bool,
}

/// Content block in a tool call result
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(super) enum ToolContent {
    Text { text: String },
    #[serde(other)]
    Other,
}
