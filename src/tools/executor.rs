//! Tool executor — dispatches tool calls to Synapse MCP or plugin subprocess

use std::sync::Arc;
use std::time::Duration;

use synapse_client::SynapseClient;
use tokio::sync::Mutex;

use crate::plugins::PluginManager;
use crate::{Error, Result};

/// Shared plugin manager type
type SharedPluginManager = Arc<Mutex<PluginManager>>;

/// Classification used to determine execution strategy within a tool batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Read-only; safe to run fully in parallel.
    Read,
    /// Mutating; serialized among other mutating tools but parallel with reads.
    Mutate,
    /// Requires user response before any tool in the batch runs.
    Interactive,
}

impl ToolKind {
    /// Classify a tool by name.
    ///
    /// Unknown tools default to `Mutate` — the safe conservative choice.
    #[must_use]
    pub fn classify(name: &str) -> Self {
        match name {
            // Read-only tools
            "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch"
            | "ListDir" | "NotebookRead" | "TaskList" | "TaskGet" => Self::Read,
            // Interactive tools
            "ask_user" | "permission" | "AskUserQuestion" | "location_request" => Self::Interactive,
            // Everything else defaults to Mutate (safe)
            _ => Self::Mutate,
        }
    }
}

/// Executes tool calls via Synapse MCP or plugin subprocess
pub struct ToolExecutor {
    synapse: Arc<SynapseClient>,
    plugin_manager: SharedPluginManager,
}

impl ToolExecutor {
    /// Create a new tool executor
    pub fn new(synapse: Arc<SynapseClient>, plugin_manager: SharedPluginManager) -> Self {
        Self {
            synapse,
            plugin_manager,
        }
    }

    /// Fetch available tools from both Synapse MCP and loaded plugins
    pub async fn list_tools(&self) -> Result<Vec<synapse_client::ToolDefinition>> {
        // Fetch Synapse MCP tools
        let mut definitions: Vec<synapse_client::ToolDefinition> = self
            .synapse
            .list_tools(None)
            .await
            .map_err(|e| Error::Tool(e.to_string()))?
            .into_iter()
            .map(synapse_client::ToolDefinition::from)
            .collect();

        // Merge plugin tools
        let pm = self.plugin_manager.lock().await;
        for (scoped_name, tool_def) in pm.tools() {
            definitions.push(synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: scoped_name,
                    description: Some(tool_def.description),
                    parameters: Some(tool_def.input_schema),
                },
            });
        }

        Ok(definitions)
    }

    /// Execute a tool call, routing to plugin subprocess or Synapse MCP
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        // Plugin tools use `plugin_id::tool_name` format
        if let Some((plugin_id, tool_name)) = name.split_once("::") {
            return self.execute_plugin(plugin_id, tool_name, arguments).await;
        }

        // Route to Synapse MCP
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));

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

        let entry = plugin.manifest.entry.as_deref().ok_or_else(|| {
            Error::Tool(format!("plugin {plugin_id} has no entry point"))
        })?;

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
        assert_eq!(ToolKind::classify("Read"), ToolKind::Read);
        assert_eq!(ToolKind::classify("Glob"), ToolKind::Read);
        assert_eq!(ToolKind::classify("Grep"), ToolKind::Read);
        assert_eq!(ToolKind::classify("WebSearch"), ToolKind::Read);
        assert_eq!(ToolKind::classify("WebFetch"), ToolKind::Read);
        assert_eq!(ToolKind::classify("Write"), ToolKind::Mutate);
        assert_eq!(ToolKind::classify("Edit"), ToolKind::Mutate);
        assert_eq!(ToolKind::classify("Bash"), ToolKind::Mutate);
        assert_eq!(ToolKind::classify("ListDir"), ToolKind::Read);
        assert_eq!(ToolKind::classify("NotebookRead"), ToolKind::Read);
        assert_eq!(ToolKind::classify("TaskList"), ToolKind::Read);
        assert_eq!(ToolKind::classify("TaskGet"), ToolKind::Read);
        assert_eq!(ToolKind::classify("ask_user"), ToolKind::Interactive);
        assert_eq!(ToolKind::classify("permission"), ToolKind::Interactive);
        assert_eq!(ToolKind::classify("AskUserQuestion"), ToolKind::Interactive);
        assert_eq!(ToolKind::classify("location_request"), ToolKind::Interactive);
        // Unknown tools default to Mutate (safe default)
        assert_eq!(ToolKind::classify("unknown_tool"), ToolKind::Mutate);
    }
}
