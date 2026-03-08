//! Built-in browser automation tools for the LLM
//!
//! Wraps `BrowserController` as agent-callable tools so the LLM can
//! navigate pages, click elements, type text, take screenshots, and
//! extract content.

use std::sync::Arc;

use base64::Engine;

use crate::tools::browser::{BrowserController, BrowserControllerConfig};
use crate::{Error, Result};

/// Built-in browser tools exposed to the LLM
pub struct BuiltinBrowserTools {
    controller: Arc<BrowserController>,
}

impl Default for BuiltinBrowserTools {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinBrowserTools {
    /// Create browser tools with default config
    #[must_use]
    pub fn new() -> Self {
        Self {
            controller: Arc::new(BrowserController::new(BrowserControllerConfig::default())),
        }
    }

    /// Create browser tools with custom config
    #[must_use]
    pub fn with_config(config: BrowserControllerConfig) -> Self {
        Self {
            controller: Arc::new(BrowserController::new(config)),
        }
    }

    /// Return tool definitions for all browser tools
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn tool_definitions() -> Vec<synapse_client::ToolDefinition> {
        vec![
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "browser_navigate".to_string(),
                    description: Some(
                        "Navigate to a URL and return the page title and text content."
                            .to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "URL to navigate to"
                            }
                        },
                        "required": ["url"]
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "browser_click".to_string(),
                    description: Some(
                        "Click an element on the current page by CSS selector.".to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "selector": {
                                "type": "string",
                                "description": "CSS selector for the element to click"
                            }
                        },
                        "required": ["selector"]
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "browser_type".to_string(),
                    description: Some(
                        "Type text into an input element identified by CSS selector.".to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "selector": {
                                "type": "string",
                                "description": "CSS selector for the input element"
                            },
                            "text": {
                                "type": "string",
                                "description": "Text to type into the element"
                            }
                        },
                        "required": ["selector", "text"]
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "browser_screenshot".to_string(),
                    description: Some(
                        "Take a screenshot of the current page or a specific URL. Returns base64-encoded PNG."
                            .to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "Optional URL to navigate to before taking screenshot"
                            }
                        }
                    })),
                },
            },
            synapse_client::ToolDefinition {
                tool_type: "function".to_owned(),
                function: synapse_client::FunctionDefinition {
                    name: "browser_extract".to_string(),
                    description: Some(
                        "Extract elements from the current page matching a CSS selector. Returns tag, text, and attributes for each match."
                            .to_string(),
                    ),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "selector": {
                                "type": "string",
                                "description": "CSS selector to query"
                            }
                        },
                        "required": ["selector"]
                    })),
                },
            },
        ]
    }

    /// Ensure the browser is launched
    async fn ensure_running(&self) -> Result<()> {
        if !self.controller.is_running().await {
            self.controller.launch().await?;
        }
        Ok(())
    }

    /// Execute a browser tool by name
    ///
    /// # Errors
    ///
    /// Returns error if the tool name is unknown or execution fails
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::default()));

        match name {
            "browser_navigate" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Tool("missing required parameter: url".to_string()))?;

                self.ensure_running().await?;
                let content = self.controller.navigate(url).await?;

                Ok(serde_json::json!({
                    "url": content.url,
                    "title": content.title,
                    "text": content.text,
                })
                .to_string())
            }
            "browser_click" => {
                let selector = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        Error::Tool("missing required parameter: selector".to_string())
                    })?;

                self.ensure_running().await?;
                self.controller.click(selector).await?;
                Ok(r#"{"status":"clicked"}"#.to_string())
            }
            "browser_type" => {
                let selector = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        Error::Tool("missing required parameter: selector".to_string())
                    })?;
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Tool("missing required parameter: text".to_string()))?;

                self.ensure_running().await?;
                self.controller.type_text(selector, text).await?;
                Ok(r#"{"status":"typed"}"#.to_string())
            }
            "browser_screenshot" => {
                let url = args.get("url").and_then(|v| v.as_str());

                self.ensure_running().await?;
                let screenshot = self.controller.screenshot(url).await?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&screenshot.data);

                Ok(serde_json::json!({
                    "format": screenshot.format,
                    "data": b64,
                    "size_bytes": screenshot.data.len(),
                })
                .to_string())
            }
            "browser_extract" => {
                let selector = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        Error::Tool("missing required parameter: selector".to_string())
                    })?;

                self.ensure_running().await?;
                let elements = self.controller.query_selector_all(selector).await?;

                let results: Vec<serde_json::Value> = elements
                    .into_iter()
                    .map(|el| {
                        let attrs: serde_json::Map<String, serde_json::Value> = el
                            .attributes
                            .into_iter()
                            .map(|(k, v)| (k, serde_json::Value::String(v)))
                            .collect();
                        serde_json::json!({
                            "tag": el.tag,
                            "text": el.text,
                            "attributes": attrs,
                        })
                    })
                    .collect();

                Ok(serde_json::json!({ "elements": results }).to_string())
            }
            _ => Err(Error::Tool(format!("unknown browser tool: {name}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_count() {
        let defs = BuiltinBrowserTools::tool_definitions();
        assert_eq!(defs.len(), 5);

        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_type"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"browser_extract"));
    }

    #[test]
    fn all_tools_have_descriptions() {
        for def in BuiltinBrowserTools::tool_definitions() {
            assert!(
                def.function.description.is_some(),
                "{} missing description",
                def.function.name
            );
        }
    }
}
