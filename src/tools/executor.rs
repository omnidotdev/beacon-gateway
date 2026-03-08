//! Tool executor — dispatches tool calls to Synapse MCP or plugin subprocess

use std::sync::Arc;
use std::time::Duration;

use agent_core::tools::ToolKind;
use synapse_client::SynapseClient;
use tokio::sync::Mutex;

use crate::mcp::McpServerManager;
use crate::plugins::PluginManager;
use crate::{Error, Result};

/// Shared plugin manager type
type SharedPluginManager = Arc<Mutex<PluginManager>>;

/// Classify a tool by name for execution strategy
///
/// Unknown tools default to `Mutate` — the safe conservative choice.
#[must_use]
pub fn classify(name: &str) -> ToolKind {
    match name {
        // Read-only tools
        "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" | "ListDir" | "NotebookRead"
        | "TaskList" | "TaskGet" | "memory_search" | "cron_list" | "cron_get"
        | "browser_screenshot" | "browser_extract" => ToolKind::Read,
        // Interactive tools
        "ask_user" | "permission" | "AskUserQuestion" | "location_request" => ToolKind::Interactive,
        // MCP server tools default to Mutate (safe conservative choice)
        _ if name.starts_with("mcp_") => ToolKind::Mutate,
        // Everything else defaults to Mutate (safe), including:
        // memory_store, memory_forget, cron_schedule, cron_cancel
        _ => ToolKind::Mutate,
    }
}

/// Executes tool calls via Synapse MCP, plugin subprocess, or direct MCP servers
pub struct ToolExecutor {
    synapse: Arc<SynapseClient>,
    plugin_manager: SharedPluginManager,
    memory_tools: Option<Arc<crate::tools::BuiltinMemoryTools>>,
    cron_tools: Option<Arc<crate::tools::BuiltinCronTools>>,
    exec_tool: Option<Arc<crate::tools::BuiltinExecTool>>,
    browser_tools: Option<Arc<crate::tools::BuiltinBrowserTools>>,
    mcp_manager: Option<Arc<McpServerManager>>,
}

impl ToolExecutor {
    /// Create a new tool executor
    pub const fn new(synapse: Arc<SynapseClient>, plugin_manager: SharedPluginManager) -> Self {
        Self {
            synapse,
            plugin_manager,
            memory_tools: None,
            cron_tools: None,
            exec_tool: None,
            browser_tools: None,
            mcp_manager: None,
        }
    }

    /// Attach built-in memory tools to this executor
    #[must_use]
    pub fn with_memory_tools(mut self, tools: Arc<crate::tools::BuiltinMemoryTools>) -> Self {
        self.memory_tools = Some(tools);
        self
    }

    /// Attach built-in cron tools to this executor
    #[must_use]
    pub fn with_cron_tools(mut self, tools: Arc<crate::tools::BuiltinCronTools>) -> Self {
        self.cron_tools = Some(tools);
        self
    }

    /// Attach built-in exec tool to this executor
    #[must_use]
    pub fn with_exec_tool(mut self, tool: Arc<crate::tools::BuiltinExecTool>) -> Self {
        self.exec_tool = Some(tool);
        self
    }

    /// Attach built-in browser tools to this executor
    #[must_use]
    pub fn with_browser_tools(mut self, tools: Arc<crate::tools::BuiltinBrowserTools>) -> Self {
        self.browser_tools = Some(tools);
        self
    }

    /// Attach MCP server manager to this executor
    #[must_use]
    pub fn with_mcp_manager(mut self, manager: Arc<McpServerManager>) -> Self {
        self.mcp_manager = Some(manager);
        self
    }

    /// Fetch available tools from both Synapse MCP and loaded plugins
    ///
    /// # Errors
    ///
    /// Returns error if Synapse tool listing fails
    pub async fn list_tools(&self) -> Result<Vec<synapse_client::ToolDefinition>> {
        // Fetch Synapse MCP tools (gracefully degrade if unavailable, e.g. embedded mode)
        let mut definitions: Vec<synapse_client::ToolDefinition> = match self
            .synapse
            .list_tools(None)
            .await
        {
            Ok(tools) => tools
                .into_iter()
                .map(synapse_client::ToolDefinition::from)
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "Synapse MCP tool listing unavailable, using built-ins only");
                Vec::new()
            }
        };

        // Merge plugin tools
        {
            let pm_guard = self.plugin_manager.lock().await;
            for (scoped_name, tool_def) in pm_guard.tools() {
                definitions.push(synapse_client::ToolDefinition {
                    tool_type: "function".to_owned(),
                    function: synapse_client::FunctionDefinition {
                        name: scoped_name,
                        description: Some(tool_def.description),
                        parameters: Some(tool_def.input_schema),
                    },
                });
            }
        }

        // Include built-in tool provider definitions
        use agent_core::tools::ToolProvider;

        if let Some(ref mt) = self.memory_tools {
            definitions.extend(
                mt.definitions()
                    .iter()
                    .map(crate::tools::to_synapse_definition),
            );
        }

        if let Some(ref ct) = self.cron_tools {
            definitions.extend(
                ct.definitions()
                    .iter()
                    .map(crate::tools::to_synapse_definition),
            );
        }

        if let Some(ref et) = self.exec_tool {
            definitions.extend(
                et.definitions()
                    .iter()
                    .map(crate::tools::to_synapse_definition),
            );
        }

        if let Some(ref bt) = self.browser_tools {
            definitions.extend(
                bt.definitions()
                    .iter()
                    .map(crate::tools::to_synapse_definition),
            );
        }

        // Include tools from direct MCP servers
        if let Some(ref mcp) = self.mcp_manager {
            for tool in mcp.all_tools().await {
                definitions.push(synapse_client::ToolDefinition {
                    tool_type: "function".to_owned(),
                    function: synapse_client::FunctionDefinition {
                        name: tool.name,
                        description: tool.description,
                        parameters: Some(tool.input_schema),
                    },
                });
            }
        }

        Ok(definitions)
    }

    /// Execute a tool call, routing to plugin subprocess or Synapse MCP
    ///
    /// # Errors
    ///
    /// Returns error if tool execution fails
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        // Route built-in memory tools
        if name.starts_with("memory_")
            && let Some(ref mt) = self.memory_tools
        {
            return mt.execute(name, arguments).await;
        }

        // Route built-in cron tools
        if name.starts_with("cron_")
            && let Some(ref ct) = self.cron_tools
        {
            return ct.execute(name, arguments).await;
        }

        // Route built-in exec tool
        if name == "Bash"
            && let Some(ref et) = self.exec_tool
        {
            return et.execute(name, arguments).await;
        }

        // Route built-in browser tools
        if name.starts_with("browser_")
            && let Some(ref bt) = self.browser_tools
        {
            return bt.execute(name, arguments).await;
        }

        // Route MCP server tools (prefixed with `mcp_`)
        if name.starts_with("mcp_")
            && let Some(ref mcp) = self.mcp_manager
        {
            let args: serde_json::Value = serde_json::from_str(arguments)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::default()));
            let result = mcp.call_tool(name, args).await.map_err(Error::Tool)?;
            if result.is_error {
                return Err(Error::Tool(result.text));
            }
            return Ok(result.text);
        }

        // Plugin tools use `plugin_id::tool_name` format
        if let Some((plugin_id, tool_name)) = name.split_once("::") {
            return self.execute_plugin(plugin_id, tool_name, arguments).await;
        }

        // Route to Synapse MCP
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::default()));

        let result = self
            .synapse
            .call_tool(name, args)
            .await
            .map_err(|e| Error::Tool(e.to_string()))?;

        Ok(result.text())
    }

    /// Execute a plugin tool via subprocess
    async fn execute_plugin(
        &self,
        plugin_id: &str,
        tool_name: &str,
        arguments: &str,
    ) -> Result<String> {
        let pm = self.plugin_manager.lock().await;

        let plugin = pm
            .get(plugin_id)
            .ok_or_else(|| Error::Tool(format!("plugin not found: {plugin_id}")))?;

        if !plugin.enabled {
            return Err(Error::Tool(format!("plugin disabled: {plugin_id}")));
        }

        let entry = plugin
            .manifest
            .entry
            .as_deref()
            .ok_or_else(|| Error::Tool(format!("plugin {plugin_id} has no entry point")))?;

        let entry_path = plugin.path.join(entry);
        // Release the lock before spawning
        drop(pm);

        run_plugin_entry(&entry_path, tool_name, arguments).await
    }
}

/// Spawn a plugin entry point as a subprocess
///
/// Detects runtime from file extension:
/// - `.js` / `.ts` -> `bun`
/// - `.py` -> `python3`
/// - anything else -> direct execution
///
/// Arguments: `<tool_name> <arguments_json>`
/// Captures stdout, 30s timeout.
async fn run_plugin_entry(
    entry_path: &std::path::Path,
    tool_name: &str,
    arguments: &str,
) -> Result<String> {
    let ext = entry_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // TODO: add "wasm" arm using Extism for sandboxed execution of untrusted plugins
    let mut cmd = match ext {
        "js" | "ts" | "mjs" | "mts" => {
            let mut c = tokio::process::Command::new("bun");
            c.arg(entry_path);
            c
        }
        "py" => {
            let mut c = tokio::process::Command::new("python3");
            c.arg(entry_path);
            c
        }
        _ => tokio::process::Command::new(entry_path),
    };

    cmd.arg(tool_name).arg(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Set working directory to plugin directory
    if let Some(parent) = entry_path.parent() {
        cmd.current_dir(parent);
    }

    let child = cmd.spawn().map_err(|e| {
        Error::Tool(format!(
            "failed to spawn plugin process {}: {e}",
            entry_path.display()
        ))
    })?;

    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| Error::Tool("plugin execution timed out (30s)".to_string()))?
        .map_err(|e| Error::Tool(format!("plugin process error: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Tool(format!(
            "plugin exited with {}: {stderr}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_tools() {
        assert_eq!(classify("Read"), ToolKind::Read);
        assert_eq!(classify("Glob"), ToolKind::Read);
        assert_eq!(classify("Grep"), ToolKind::Read);
        assert_eq!(classify("WebSearch"), ToolKind::Read);
        assert_eq!(classify("WebFetch"), ToolKind::Read);
        assert_eq!(classify("Write"), ToolKind::Mutate);
        assert_eq!(classify("Edit"), ToolKind::Mutate);
        assert_eq!(classify("Bash"), ToolKind::Mutate);
        assert_eq!(classify("ListDir"), ToolKind::Read);
        assert_eq!(classify("NotebookRead"), ToolKind::Read);
        assert_eq!(classify("TaskList"), ToolKind::Read);
        assert_eq!(classify("TaskGet"), ToolKind::Read);
        assert_eq!(classify("ask_user"), ToolKind::Interactive);
        assert_eq!(classify("permission"), ToolKind::Interactive);
        assert_eq!(classify("AskUserQuestion"), ToolKind::Interactive);
        assert_eq!(classify("location_request"), ToolKind::Interactive);
        // Memory tools
        assert_eq!(classify("memory_search"), ToolKind::Read);
        assert_eq!(classify("memory_store"), ToolKind::Mutate);
        assert_eq!(classify("memory_forget"), ToolKind::Mutate);
        // Cron tools
        assert_eq!(classify("cron_list"), ToolKind::Read);
        assert_eq!(classify("cron_get"), ToolKind::Read);
        assert_eq!(classify("cron_schedule"), ToolKind::Mutate);
        assert_eq!(classify("cron_cancel"), ToolKind::Mutate);
        // Browser tools
        assert_eq!(classify("browser_screenshot"), ToolKind::Read);
        assert_eq!(classify("browser_extract"), ToolKind::Read);
        assert_eq!(classify("browser_navigate"), ToolKind::Mutate);
        assert_eq!(classify("browser_click"), ToolKind::Mutate);
        assert_eq!(classify("browser_type"), ToolKind::Mutate);
        // Unknown tools default to Mutate (safe default)
        assert_eq!(classify("unknown_tool"), ToolKind::Mutate);
    }
}
