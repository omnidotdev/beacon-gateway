//! MCP server lifecycle manager
//!
//! Manages multiple MCP server processes, aggregates their tools,
//! and routes tool calls to the correct server

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::client::McpClient;
use super::types::{McpServerConfig, McpTool, McpToolResult};

/// Manages multiple MCP server connections
pub struct McpServerManager {
    /// Running server clients, keyed by server name
    servers: Mutex<HashMap<String, Arc<McpClient>>>,
    /// Map from tool name -> server name for routing
    tool_routes: Mutex<HashMap<String, String>>,
}

impl McpServerManager {
    /// Create an empty manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            tool_routes: Mutex::new(HashMap::new()),
        }
    }

    /// Start all configured MCP servers
    ///
    /// Failures are logged but do not block other servers from starting
    pub async fn start_all(&self, configs: &[McpServerConfig]) {
        for config in configs {
            if let Err(e) = self.start_server(config.clone()).await {
                tracing::error!(
                    server = %config.name,
                    error = %e,
                    "failed to start MCP server"
                );
            }
        }
    }

    /// Start a single MCP server and register its tools
    ///
    /// # Errors
    ///
    /// Returns error if the server fails to start
    pub async fn start_server(&self, config: McpServerConfig) -> Result<(), String> {
        let name = config.name.clone();

        // Check for duplicate
        if self.servers.lock().await.contains_key(&name) {
            return Err(format!("MCP server '{name}' already running"));
        }

        let client = McpClient::start(config).await?;
        let tools = client.tools().await;
        let client = Arc::new(client);

        // Register tool routes
        {
            let mut routes = self.tool_routes.lock().await;
            for tool in &tools {
                let scoped = format!("mcp_{}/{}", name, tool.name);
                routes.insert(scoped, name.clone());
                // Also register unscoped for servers with unique tool names
                routes.entry(tool.name.clone()).or_insert(name.clone());
            }
        }

        self.servers.lock().await.insert(name, client);

        Ok(())
    }

    /// Stop a specific server
    pub async fn stop_server(&self, name: &str) {
        let client = self.servers.lock().await.remove(name);
        if let Some(client) = client {
            // Remove tool routes for this server
            let mut routes = self.tool_routes.lock().await;
            routes.retain(|_, v| v != name);

            client.stop().await;
            tracing::info!(server = %name, "MCP server stopped");
        }
    }

    /// Stop all servers
    pub async fn stop_all(&self) {
        let servers: Vec<(String, Arc<McpClient>)> =
            self.servers.lock().await.drain().collect();

        for (name, client) in servers {
            client.stop().await;
            tracing::info!(server = %name, "MCP server stopped");
        }

        self.tool_routes.lock().await.clear();
    }

    /// Get all tool definitions from all running servers
    ///
    /// Tool names are scoped as `mcp_{server_name}/{tool_name}`
    pub async fn all_tools(&self) -> Vec<McpTool> {
        let servers = self.servers.lock().await;
        let mut all = Vec::new();

        for (name, client) in servers.iter() {
            for tool in client.tools().await {
                all.push(McpTool {
                    name: format!("mcp_{}/{}", name, tool.name),
                    description: tool.description,
                    input_schema: tool.input_schema,
                    server_name: name.clone(),
                });
            }
        }

        all
    }

    /// Check if a tool name belongs to an MCP server
    pub async fn has_tool(&self, name: &str) -> bool {
        self.tool_routes.lock().await.contains_key(name)
    }

    /// Call a tool, routing to the appropriate server
    ///
    /// Accepts both scoped (`mcp_servername/toolname`) and unscoped names
    ///
    /// # Errors
    ///
    /// Returns error if the server is not found or the tool call fails
    pub async fn call_tool(
        &self,
        scoped_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        // Parse server name and tool name from scoped format
        let (server_name, tool_name) = if let Some(rest) = scoped_name.strip_prefix("mcp_") {
            rest.split_once('/')
                .ok_or_else(|| format!("invalid MCP tool name: {scoped_name}"))?
        } else {
            // Look up in routes
            let routes = self.tool_routes.lock().await;
            let server = routes
                .get(scoped_name)
                .ok_or_else(|| format!("no MCP server for tool: {scoped_name}"))?
                .clone();
            drop(routes);
            // The tool_name is the scoped_name itself
            return self.call_on_server(&server, scoped_name, arguments).await;
        };

        self.call_on_server(server_name, tool_name, arguments).await
    }

    /// Call a tool on a specific server
    async fn call_on_server(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let servers = self.servers.lock().await;
        let client = servers
            .get(server_name)
            .ok_or_else(|| format!("MCP server not found: {server_name}"))?;
        let client = Arc::clone(client);
        drop(servers);

        client.call_tool(tool_name, arguments).await
    }

    /// List running server names
    pub async fn server_names(&self) -> Vec<String> {
        self.servers.lock().await.keys().cloned().collect()
    }
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new()
    }
}
