//! MCP client - communicates with a single MCP server over stdio

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::types::{
    InitializeResult, JsonRpcRequest, JsonRpcResponse, McpServerConfig, McpTool, McpToolResult,
    ToolCallResult, ToolContent, ToolsListResult,
};

/// Shared state for pending request tracking
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

/// Client for a single MCP server process
pub struct McpClient {
    config: McpServerConfig,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicU64,
    child: Mutex<Option<Child>>,
    tools: Mutex<Vec<McpTool>>,
}

impl McpClient {
    /// Spawn the MCP server process and perform the initialize handshake
    ///
    /// # Errors
    ///
    /// Returns error if the process fails to spawn or initialize
    pub async fn start(config: McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn MCP server '{}': {e}", config.name))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture stdout".to_string())?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Spawn reader task that dispatches responses to waiting callers
        let pending_clone = Arc::clone(&pending);
        let server_name = config.name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcResponse>(&line) {
                    Ok(resp) => {
                        if let Some(id) = resp.id {
                            let mut map = pending_clone.lock().await;
                            if let Some(tx) = map.remove(&id) {
                                let _ = tx.send(resp);
                            }
                        }
                        // Notifications from server (no id) are logged and dropped
                    }
                    Err(e) => {
                        tracing::trace!(
                            server = %server_name,
                            error = %e,
                            "non-JSON-RPC line from MCP server"
                        );
                    }
                }
            }
            tracing::debug!(server = %server_name, "MCP server stdout closed");
        });

        let client = Self {
            config,
            stdin: Mutex::new(Some(stdin)),
            pending,
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            tools: Mutex::new(Vec::new()),
        };

        // Perform MCP initialize handshake
        let init_result = client
            .send_request::<InitializeResult>(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "beacon-gateway",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;

        tracing::info!(
            server = %client.config.name,
            protocol = %init_result.protocol_version,
            server_name = ?init_result.server_info.as_ref().map(|s| &s.name),
            "MCP server initialized"
        );

        // Send initialized notification
        client
            .send_notification("notifications/initialized", None)
            .await?;

        // Fetch available tools
        client.refresh_tools().await?;

        Ok(client)
    }

    /// Fetch the tool list from the server and cache it
    ///
    /// # Errors
    ///
    /// Returns error if the tools/list request fails
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let result = self
            .send_request::<ToolsListResult>("tools/list", None)
            .await?;

        let tools: Vec<McpTool> = result
            .tools
            .into_iter()
            .map(|t| McpTool {
                name: t.name,
                description: t.description,
                input_schema: t.input_schema,
                server_name: self.config.name.clone(),
            })
            .collect();

        tracing::info!(
            server = %self.config.name,
            count = tools.len(),
            "fetched MCP tools"
        );

        *self.tools.lock().await = tools;
        Ok(())
    }

    /// Get cached tool definitions
    pub async fn tools(&self) -> Vec<McpTool> {
        self.tools.lock().await.clone()
    }

    /// Call a tool on this server
    ///
    /// # Errors
    ///
    /// Returns error if the tool call fails
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let result = self
            .send_request::<ToolCallResult>(
                "tools/call",
                Some(serde_json::json!({
                    "name": name,
                    "arguments": arguments
                })),
            )
            .await?;

        let text = result
            .content
            .into_iter()
            .filter_map(|c| match c {
                ToolContent::Text { text } => Some(text),
                ToolContent::Other => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(McpToolResult {
            text,
            is_error: result.is_error,
        })
    }

    /// Stop the MCP server process
    pub async fn stop(&self) {
        // Drop stdin to signal EOF
        let _ = self.stdin.lock().await.take();

        if let Some(mut child) = self.child.lock().await.take() {
            tokio::select! {
                _ = child.wait() => {}
                () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    let _ = child.kill().await;
                }
            }
        }
    }

    /// Send a JSON-RPC request and wait for the response
    async fn send_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let mut line =
            serde_json::to_string(&request).map_err(|e| format!("serialize error: {e}"))?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            let writer = stdin
                .as_mut()
                .ok_or_else(|| "MCP server stdin closed".to_string())?;
            writer
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("write to MCP server failed: {e}"))?;
            writer
                .flush()
                .await
                .map_err(|e| format!("flush to MCP server failed: {e}"))?;
        }

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| format!("MCP request '{method}' timed out after 30s"))?
            .map_err(|_| "MCP response channel dropped".to_string())?;

        if let Some(error) = response.error {
            return Err(format!("MCP error ({}): {}", error.code, error.message));
        }

        let result = response
            .result
            .ok_or_else(|| "MCP response missing result".to_string())?;

        serde_json::from_value(result).map_err(|e| format!("MCP result parse error: {e}"))
    }

    /// Send a JSON-RPC notification (no response expected)
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(serde_json::json!({}))
        });

        let mut line = serde_json::to_string(&msg).map_err(|e| format!("serialize error: {e}"))?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        let writer = stdin
            .as_mut()
            .ok_or_else(|| "MCP server stdin closed".to_string())?;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write notification failed: {e}"))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("flush notification failed: {e}"))?;

        Ok(())
    }
}
