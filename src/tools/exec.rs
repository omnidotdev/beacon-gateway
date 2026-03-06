//! Built-in shell execution tool for the LLM
//!
//! Provides a sandboxed `Bash` tool that runs commands via `/bin/sh -c`,
//! captures stdout/stderr, enforces timeouts, and augments `PATH`

use std::path::PathBuf;
use std::time::Duration;

use tokio::process::Command;

use crate::{Error, Result};

/// Default command timeout in seconds
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Maximum allowed timeout in seconds
const MAX_TIMEOUT_SECS: u64 = 600;

/// Built-in shell execution tool for LLM command execution
#[derive(Debug, Clone)]
pub struct BuiltinExecTool {
    /// Working directory for command execution
    working_dir: PathBuf,
    /// Additional PATH entries prepended to the environment
    extra_path: Vec<PathBuf>,
}

impl BuiltinExecTool {
    /// Create a new exec tool with the given working directory and extra PATH entries
    #[must_use]
    pub fn new(working_dir: PathBuf, extra_path: Vec<PathBuf>) -> Self {
        Self {
            working_dir,
            extra_path,
        }
    }

    /// Return the tool definition for the `Bash` tool
    #[must_use]
    pub fn tool_definitions() -> Vec<synapse_client::ToolDefinition> {
        vec![synapse_client::ToolDefinition {
            tool_type: "function".to_owned(),
            function: synapse_client::FunctionDefinition {
                name: "Bash".to_string(),
                description: Some(
                    "Execute a shell command and return its output. Commands run via /bin/sh -c."
                        .to_string(),
                ),
                parameters: Some(serde_json::json!({
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
                })),
            },
        }]
    }

    /// Build the augmented PATH value
    fn augmented_path(&self) -> String {
        let system_path = std::env::var("PATH").unwrap_or_default();
        let extra: Vec<String> = self
            .extra_path
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        if extra.is_empty() {
            system_path
        } else {
            format!("{}:{system_path}", extra.join(":"))
        }
    }

    /// Execute the `Bash` tool
    ///
    /// # Errors
    ///
    /// Returns error if arguments are malformed, the command is missing,
    /// or the process fails to spawn
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        if name != "Bash" {
            return Err(Error::Tool(format!("unknown exec tool: {name}")));
        }

        #[derive(serde::Deserialize)]
        struct ExecArgs {
            command: Option<String>,
            #[serde(default)]
            timeout: Option<u64>,
        }

        let args: ExecArgs = serde_json::from_str(arguments)
            .map_err(|e| Error::Tool(format!("Bash: invalid arguments: {e}")))?;

        let command = args
            .command
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| Error::Tool("Bash: `command` parameter is required".to_string()))?;

        let timeout_secs = args
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        tracing::debug!(command = %command, timeout_secs, "Bash: executing command");

        let child = {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c")
                .arg(&command)
                .current_dir(&self.working_dir)
                .env("PATH", self.augmented_path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);

            // Create a new process group so the entire tree is killed on timeout
            #[cfg(unix)]
            cmd.process_group(0);

            cmd.spawn()
                .map_err(|e| Error::Tool(format!("Bash: failed to spawn: {e}")))?
        };

        let result =
            tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
                .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);

                tracing::debug!(
                    exit_code = code,
                    stdout_len = stdout.len(),
                    stderr_len = stderr.len(),
                    "Bash: command completed"
                );

                let response = serde_json::json!({
                    "exit_code": code,
                    "stdout": stdout.as_ref(),
                    "stderr": stderr.as_ref(),
                });

                Ok(response.to_string())
            }
            Ok(Err(e)) => Err(Error::Tool(format!("Bash: process error: {e}"))),
            Err(_) => {
                tracing::warn!(command = %command, timeout_secs, "Bash: command timed out");
                Err(Error::Tool(format!(
                    "Bash: command timed out after {timeout_secs}s"
                )))
            }
        }
    }
}

impl Default for BuiltinExecTool {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));

        let extra_path = vec![
            home.join(".bun/bin"),
            home.join(".local/bin"),
            home.join(".cargo/bin"),
        ];

        Self::new(home, extra_path)
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

        // No command field
        let result = tool.execute("Bash", r#"{}"#).await;
        assert!(result.is_err());

        // Empty command
        let result = tool.execute("Bash", r#"{"command": "  "}"#).await;
        assert!(result.is_err());
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
