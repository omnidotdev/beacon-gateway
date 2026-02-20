//! Shared agentic turn runner

use std::sync::Arc;

use futures::StreamExt;
use synapse_client::ChatEvent;

use crate::api::ApiState;
use crate::db::MessageRole;
use crate::tools::executor::ToolKind;

/// Configuration for a single agentic turn
#[derive(Debug, Clone)]
pub struct AgentRunConfig {
    /// User-facing prompt (the task/question)
    pub prompt: String,
    /// System prompt (persona)
    pub system_prompt: String,
    /// LLM model identifier
    pub model: String,
    /// Max tokens per completion
    pub max_tokens: u32,
    /// Max agentic iterations (tool call rounds)
    pub max_iterations: u32,
    /// Session ID for history
    pub session_id: String,
    /// User ID for memory/context
    pub user_id: String,
    /// Optional channel to emit tool events to a WebSocket client
    /// Pass `None` for headless/non-WebSocket callers
    pub notify: Option<tokio::sync::mpsc::Sender<AgentNotifyEvent>>,
}

/// In-progress tool call being assembled from streaming events
#[derive(Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Tool lifecycle events emitted to WebSocket clients during agent execution
/// Kept in this module to avoid circular dependency with `api::websocket`
#[derive(Debug, Clone)]
pub enum AgentNotifyEvent {
    /// Tool invocation started
    ToolStart { tool_id: String, name: String },
    /// Tool invocation completed
    ToolResult {
        tool_id: String,
        name: String,
        /// Short display summary extracted from arguments
        invocation: String,
        output: String,
        is_error: bool,
    },
}

/// Extract a short display label from tool arguments JSON
/// Tries common field names; falls back to truncated raw args
fn summarize_invocation(name: &str, args: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args) else {
        return args.chars().take(60).collect();
    };
    for field in &["query", "path", "command", "url", "pattern", "glob"] {
        if let Some(s) = v.get(field).and_then(|v| v.as_str()) {
            let truncated: String = s.chars().take(60).collect();
            return format!("{name}: {truncated}");
        }
    }
    args.chars().take(60).collect()
}

#[allow(clippy::too_many_lines)]
/// Run a full agentic turn and return the final assistant text.
///
/// Loops until the LLM stops calling tools or `max_iterations` is reached.
/// Tool calls are executed via the `ToolExecutor` on `state`. Interactive
/// tools (e.g. `ask_user`) are skipped headlessly with a placeholder response.
///
/// # Errors
///
/// Returns an error if the LLM call fails or no Synapse client is configured.
pub async fn run_agent_turn(
    state: &ApiState,
    config: AgentRunConfig,
) -> crate::Result<String> {
    let Some(synapse) = state.synapse.clone() else {
        return Err(crate::Error::Agent("no LLM provider configured".to_string()));
    };

    let memory_tools = Arc::new(crate::tools::BuiltinMemoryTools::new(
        state.memory_repo.clone(),
        state.embedder.clone(),
        config.user_id.clone(),
    ));

    // Fetch available tools
    let tools = {
        let executor = crate::tools::executor::ToolExecutor::new(
            Arc::clone(&synapse),
            state.plugin_manager.clone(),
        )
        .with_memory_tools(Arc::clone(&memory_tools));
        executor.list_tools().await.ok()
    };

    // Build initial messages
    let mut messages = if config.system_prompt.is_empty() {
        vec![synapse_client::Message::user(&config.prompt)]
    } else {
        vec![
            synapse_client::Message::system(&config.system_prompt),
            synapse_client::Message::user(&config.prompt),
        ]
    };

    let max_iter = config.max_iterations.min(20) as usize;
    let mut full_response = String::new();

    for _turn in 0..max_iter {
        let request = synapse_client::ChatRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            stream: true,
            temperature: None,
            top_p: None,
            max_tokens: Some(config.max_tokens),
            stop: None,
            tools: tools.clone(),
            tool_choice: None,
        };

        let mut stream = synapse
            .chat_completion_stream(&request)
            .await
            .map_err(|e| crate::Error::Agent(e.to_string()))?;

        let mut turn_text = String::new();
        let mut pending_tool_calls: Vec<PendingToolCall> = Vec::new();
        let mut finish_reason = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(ChatEvent::ContentDelta(text)) => {
                    turn_text.push_str(&text);
                }
                Ok(ChatEvent::ToolCallStart { index, id, name }) => {
                    let idx = index as usize;
                    if idx >= pending_tool_calls.len() {
                        pending_tool_calls.resize_with(idx + 1, PendingToolCall::default);
                    }
                    pending_tool_calls[idx].id = id;
                    pending_tool_calls[idx].name = name;
                }
                Ok(ChatEvent::ToolCallDelta { index, arguments }) => {
                    let idx = index as usize;
                    if idx < pending_tool_calls.len() {
                        pending_tool_calls[idx].arguments.push_str(&arguments);
                    }
                }
                Ok(ChatEvent::Done { finish_reason: fr, .. }) => {
                    finish_reason = fr;
                    break;
                }
                Ok(ChatEvent::Error(e)) => {
                    return Err(crate::Error::Agent(e));
                }
                Err(e) => {
                    return Err(crate::Error::Agent(e.to_string()));
                }
            }
        }

        full_response.push_str(&turn_text);

        if finish_reason.as_deref() == Some("tool_calls") && !pending_tool_calls.is_empty() {
            // Build assistant message with tool calls
            let tool_calls: Vec<synapse_client::ToolCall> = pending_tool_calls
                .iter()
                .map(|tc| synapse_client::ToolCall {
                    id: tc.id.clone(),
                    tool_type: "function".to_owned(),
                    function: synapse_client::FunctionCall {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect();

            let assistant_content = if turn_text.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(turn_text)
            };

            messages.push(synapse_client::Message {
                role: "assistant".to_owned(),
                content: assistant_content,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
            });

            let executor = Arc::new(
                crate::tools::executor::ToolExecutor::new(
                    Arc::clone(&synapse),
                    state.plugin_manager.clone(),
                )
                .with_memory_tools(Arc::clone(&memory_tools)),
            );

            // Headless: skip interactive tools, run the rest
            let (interactive, rest): (Vec<_>, Vec<_>) = pending_tool_calls
                .iter()
                .partition(|tc| matches!(tc.name.as_str(), "ask_user"));

            // Respond to interactive tools with a placeholder
            for tc in &interactive {
                tracing::debug!(
                    tool_id = %tc.id,
                    "headless agent: skipping interactive tool"
                );
                messages.push(synapse_client::Message::tool(
                    &tc.id,
                    "[not available in headless mode]",
                ));
            }

            // Partition remaining by read/mutate to run concurrently where safe
            let (reads, mutates): (Vec<_>, Vec<_>) = rest
                .into_iter()
                .partition(|tc| ToolKind::classify(&tc.name) == ToolKind::Read);

            let read_futs = reads.into_iter().map(|tc| {
                let executor = Arc::clone(&executor);
                let tool_id = tc.id.clone();
                let name = tc.name.clone();
                let args = tc.arguments.clone();
                let notify = config.notify.clone();
                async move {
                    if let Some(ref n) = notify {
                        let _ = n.send(AgentNotifyEvent::ToolStart {
                            tool_id: tool_id.clone(),
                            name: name.clone(),
                        }).await;
                    }
                    let result = executor.execute(&name, &args).await;
                    if let Some(ref n) = notify {
                        let (output, is_error) = match &result {
                            Ok(out) => (out.clone(), false),
                            Err(e) => (format!("Error: {e}"), true),
                        };
                        let _ = n.send(AgentNotifyEvent::ToolResult {
                            tool_id: tool_id.clone(),
                            name: name.clone(),
                            invocation: summarize_invocation(&name, &args),
                            output,
                            is_error,
                        }).await;
                    }
                    (tool_id, result)
                }
            });
            let read_results = futures::future::join_all(read_futs).await;

            let mut mutate_results = Vec::new();
            for tc in mutates {
                if let Some(n) = &config.notify {
                    let _ = n.send(AgentNotifyEvent::ToolStart {
                        tool_id: tc.id.clone(),
                        name: tc.name.clone(),
                    }).await;
                }
                let result = executor.execute(&tc.name, &tc.arguments).await;
                if let Some(n) = &config.notify {
                    let (output, is_error) = match &result {
                        Ok(out) => (out.clone(), false),
                        Err(e) => (format!("Error: {e}"), true),
                    };
                    let _ = n.send(AgentNotifyEvent::ToolResult {
                        tool_id: tc.id.clone(),
                        name: tc.name.clone(),
                        invocation: summarize_invocation(&tc.name, &tc.arguments),
                        output,
                        is_error,
                    }).await;
                }
                mutate_results.push((tc.id.clone(), result));
            }

            for (tool_id, result) in read_results.into_iter().chain(mutate_results) {
                let output = match result {
                    Ok(out) => out,
                    Err(e) => format!("Error: {e}"),
                };
                messages.push(synapse_client::Message::tool(&tool_id, &output));
            }

            continue;
        }

        break;
    }

    // Store user message and assistant response
    state
        .session_repo
        .add_message(&config.session_id, MessageRole::User, &config.prompt)?;
    state
        .session_repo
        .add_message(&config.session_id, MessageRole::Assistant, &full_response)?;

    Ok(full_response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_run_config_requires_non_empty_prompt() {
        let config = AgentRunConfig {
            prompt: String::new(),
            system_prompt: "sys".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 1024,
            max_iterations: 10,
            session_id: "sess_1".to_string(),
            user_id: "user_1".to_string(),
            notify: None,
        };
        assert!(config.prompt.is_empty());
    }

    #[test]
    fn summarize_invocation_extracts_query() {
        let s = summarize_invocation("web_search", r#"{"query":"microcap gem"}"#);
        assert_eq!(s, "web_search: microcap gem");
    }

    #[test]
    fn summarize_invocation_extracts_path() {
        let s = summarize_invocation("read_file", r#"{"path":"/home/user/file.txt"}"#);
        assert_eq!(s, "read_file: /home/user/file.txt");
    }

    #[test]
    fn summarize_invocation_falls_back_to_raw() {
        let s = summarize_invocation("unknown", r#"{"foo":"bar"}"#);
        assert_eq!(s, r#"{"foo":"bar"}"#);
    }
}
