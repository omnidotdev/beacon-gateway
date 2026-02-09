//! Hook execution via subprocess

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::types::{HookEvent, HookResult};

/// Default timeout for hook execution
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Execute a hook handler
///
/// Passes event as JSON on stdin, expects JSON result on stdout
pub async fn execute_hook(
    handler_path: &Path,
    event: &HookEvent,
    hook_timeout: Option<Duration>,
) -> Result<HookResult, String> {
    let timeout_duration = hook_timeout.unwrap_or(DEFAULT_TIMEOUT);

    // Serialize event to JSON
    let event_json = serde_json::to_string(event)
        .map_err(|e| format!("failed to serialize event: {e}"))?;

    // Determine how to run the handler
    let (program, args) = determine_executor(handler_path)?;

    // Spawn process
    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(handler_path.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn hook: {e}"))?;

    // Write event to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(event_json.as_bytes())
            .await
            .map_err(|e| format!("failed to write to hook stdin: {e}"))?;
    }

    // Wait for completion with timeout
    let output = timeout(timeout_duration, child.wait_with_output())
        .await
        .map_err(|_| format!("hook timed out after {timeout_duration:?}"))?
        .map_err(|e| format!("hook execution failed: {e}"))?;

    // Log stderr if present
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!(hook = %handler_path.display(), stderr = %stderr, "hook stderr");
    }

    // Check exit status
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(format!("hook exited with code {code}"));
    }

    // Parse stdout as JSON result
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        // Empty output means no changes
        return Ok(HookResult::default());
    }

    serde_json::from_str(&stdout)
        .map_err(|e| format!("failed to parse hook output: {e}"))
}

/// Determine how to execute the handler based on extension
fn determine_executor(handler_path: &Path) -> Result<(String, Vec<String>), String> {
    let extension = handler_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let path_str = handler_path
        .to_str()
        .ok_or("invalid handler path")?
        .to_string();

    match extension {
        "py" => Ok(("python3".to_string(), vec![path_str])),
        "js" => Ok(("node".to_string(), vec![path_str])),
        "ts" => Ok(("bun".to_string(), vec!["run".to_string(), path_str])),
        "rb" => Ok(("ruby".to_string(), vec![path_str])),
        "sh" => Ok(("bash".to_string(), vec![path_str])),
        "" => {
            // No extension, assume executable binary or script with shebang
            Ok((path_str, vec![]))
        }
        _ => Err(format!("unknown handler extension: .{extension}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_executor_python() {
        let path = Path::new("/hooks/my-hook/handler.py");
        let (prog, args) = determine_executor(path).unwrap();
        assert_eq!(prog, "python3");
        assert_eq!(args, vec!["/hooks/my-hook/handler.py"]);
    }

    #[test]
    fn test_determine_executor_binary() {
        let path = Path::new("/hooks/my-hook/handler");
        let (prog, args) = determine_executor(path).unwrap();
        assert_eq!(prog, "/hooks/my-hook/handler");
        assert!(args.is_empty());
    }

    #[test]
    fn test_determine_executor_typescript() {
        let path = Path::new("/hooks/my-hook/handler.ts");
        let (prog, args) = determine_executor(path).unwrap();
        assert_eq!(prog, "bun");
        assert_eq!(args, vec!["run", "/hooks/my-hook/handler.ts"]);
    }
}
