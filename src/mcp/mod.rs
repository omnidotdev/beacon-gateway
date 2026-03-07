//! MCP (Model Context Protocol) client for direct server management
//!
//! Spawns and manages MCP servers over stdio transport, aggregating their
//! tools into the gateway's tool executor

mod client;
mod manager;
mod types;

pub use client::McpClient;
pub use manager::McpServerManager;
pub use types::{McpServerConfig, McpTool, McpToolResult};
