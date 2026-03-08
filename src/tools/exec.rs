//! Built-in shell execution tool for the LLM
//!
//! Wraps `agent_core::tools::shell::ShellTool` and adds
//! Synapse-compatible tool definitions and JSON argument parsing

use std::path::PathBuf;

use agent_core::tools::shell::ShellTool;
use agent_core::tools::{ToolKind, ToolProvider};

use crate::{Error, Result};

/// Built-in shell execution tool for LLM command execution
#[derive(Debug, Clone, Default)]
pub struct BuiltinExecTool {
    inner: ShellTool,
}

impl BuiltinExecTool {
    /// Create a new exec tool with the given working directory and extra PATH entries
    #[must_use]
    pub const fn new(working_dir: PathBuf, extra_path: Vec<PathBuf>) -> Self {
        Self {
            inner: ShellTool::new(working_dir, extra_path),
        }
    }

    /// Return the tool definition for the `Bash` tool
    #[must_use]
    pub fn tool_definitions() -> Vec<synapse_client::ToolDefinition> {
        let provider = Self::default();
        provider
            .definitions()
            .iter()
            .map(crate::tools::to_synapse_definition)
            .collect()
    }

    /// Execute the `Bash` tool from LLM JSON arguments
    ///
    /// # Errors
    ///
    /// Returns error if arguments are malformed or execution fails
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        ToolProvider::execute(self, name, arguments)
            .await
            .map_err(|e| Error::Tool(e.to_string()))
    }
}

#[async_trait::async_trait]
impl ToolProvider for BuiltinExecTool {
    fn definitions(&self) -> Vec<agent_core::types::Tool> {
        vec![agent_core::types::Tool {
            name: "Bash".to_string(),
            description:
                "Execute a shell command and return its output. Commands run via /bin/sh -c."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120, max: 600)"
                    }
                },
                "required": ["command"]
            }),
        }]
    }

    async fn execute(&self, name: &str, arguments: &str) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct ExecArgs {
            command: Option<String>,
            #[serde(default)]
            timeout: Option<u64>,
        }

        if name != "Bash" {
            anyhow::bail!("unknown exec tool: {name}");
        }

        let args: ExecArgs = serde_json::from_str(arguments)
            .map_err(|e| anyhow::anyhow!("Bash: invalid arguments: {e}"))?;

        let command = args
            .command
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Bash: `command` parameter is required"))?;

        let output = self
            .inner
            .execute(&command, args.timeout)
            .await
            .map_err(|e| anyhow::anyhow!("Bash: {e}"))?;

        let response = serde_json::json!({
            "exit_code": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr,
        });

        Ok(response.to_string())
    }

    fn kind(&self, _name: &str) -> ToolKind {
        ToolKind::Mutate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> BuiltinExecTool {
        BuiltinExecTool::new(PathBuf::from("/tmp"), vec![])
    }

    #[tokio::test]
    async fn simple_command_returns_stdout() {
        let tool = make_tool();
        let result = tool
            .execute("Bash", r#"{"command": "echo hello"}"#)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["exit_code"], 0);
        assert!(v["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn stderr_is_captured() {
        let tool = make_tool();
        let result = tool
            .execute("Bash", r#"{"command": "echo err >&2"}"#)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(v["stderr"].as_str().unwrap().contains("err"));
    }

    #[tokio::test]
    async fn exit_code_is_reported() {
        let tool = make_tool();
        let result = tool
            .execute("Bash", r#"{"command": "exit 42"}"#)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["exit_code"], 42);
    }

    #[tokio::test]
    async fn timeout_is_enforced() {
        let tool = make_tool();
        let result = tool
            .execute("Bash", r#"{"command": "sleep 10", "timeout": 1}"#)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "error: {err}");
    }

    #[tokio::test]
    async fn missing_command_is_rejected() {
        let tool = make_tool();
        assert!(tool.execute("Bash", r#"{}"#).await.is_err());
        assert!(tool.execute("Bash", r#"{"command": "  "}"#).await.is_err());
    }

    #[test]
    fn tool_definition_is_valid() {
        let defs = BuiltinExecTool::tool_definitions();
        assert_eq!(defs.len(), 1);

        let def = &defs[0];
        assert_eq!(def.function.name, "Bash");
        assert_eq!(def.tool_type, "function");

        let params = def.function.parameters.as_ref().unwrap();
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "command"));

        let props = &params["properties"];
        assert!(props.get("command").is_some());
        assert!(props.get("timeout").is_some());
        assert_eq!(props["command"]["type"], "string");
        assert_eq!(props["timeout"]["type"], "integer");
    }
}
