//! Tool management for beacon gateway

mod browser;
mod browser_tools;
mod cron;
pub mod exec;
pub mod executor;
pub use agent_core::tools::loop_detection::{LoopDetector, LoopSeverity};
pub use agent_core::tools::{ToolKind, ToolProvider};
pub mod memory;
mod sessions;
mod web;

pub use agent_core::tools::policy::{ToolPolicy, ToolPolicyConfig, ToolProfile};
pub use browser::{
    BrowserController, BrowserControllerConfig, BrowserError, ElementInfo, PageContent, Screenshot,
};
pub use browser_tools::BuiltinBrowserTools;
pub use cron::{BuiltinCronTools, CronTools, ScheduleInfo, ScheduleParams};
pub use exec::BuiltinExecTool;
pub use memory::BuiltinMemoryTools;
pub use sessions::{MessageInfo, SessionInfo, SessionTools};
pub use web::{
    Article, SearchProvider, SearchResult, WebFetchTool, WebResponse, WebSearchTool,
    extract_article,
};

/// Convert an agent-core `Tool` to a synapse `ToolDefinition`
#[must_use]
pub fn to_synapse_definition(tool: &agent_core::types::Tool) -> synapse_client::ToolDefinition {
    synapse_client::ToolDefinition {
        tool_type: "function".to_owned(),
        function: synapse_client::FunctionDefinition {
            name: tool.name.clone(),
            description: Some(tool.description.clone()),
            parameters: Some(tool.input_schema.clone()),
        },
    }
}

/// Format a short display summary for a tool invocation.
///
/// Extracts a meaningful key field (command, path, pattern, URL) and truncates to 60 chars.
#[must_use]
pub fn format_invocation(name: &str, arguments: &str) -> String {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return name.to_string();
    };

    let key = match name {
        "Bash" | "shell" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        _ => {
            if let Some(val) = args
                .as_object()
                .and_then(|o| o.values().find(|v| v.is_string()))
            {
                return val.as_str().unwrap_or(name).chars().take(60).collect();
            }
            return name.to_string();
        }
    };

    args.get(key).and_then(|v| v.as_str()).map_or_else(
        || name.to_string(),
        |s| s.chars().take(60).collect::<String>(),
    )
}
