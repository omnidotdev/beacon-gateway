//! Tool management for beacon gateway

mod browser;
mod cron;
pub mod executor;
mod policy;
mod sessions;
mod web;

pub use browser::{BrowserController, BrowserControllerConfig, ElementInfo, PageContent, Screenshot};
pub use cron::{CronTools, ScheduleInfo, ScheduleParams};
pub use policy::{ToolPolicy, ToolPolicyConfig, ToolProfile};
pub use sessions::{MessageInfo, SessionInfo, SessionTools};
pub use web::{extract_article, Article, SearchProvider, SearchResult, WebFetchTool, WebResponse, WebSearchTool};

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
                return val
                    .as_str()
                    .unwrap_or(name)
                    .chars()
                    .take(60)
                    .collect();
            }
            return name.to_string();
        }
    };

    args.get(key)
        .and_then(|v| v.as_str())
        .map_or_else(|| name.to_string(), |s| s.chars().take(60).collect::<String>())
}
