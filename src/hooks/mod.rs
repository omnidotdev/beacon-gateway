//! Hook system for message lifecycle events
//!
//! Supports both built-in handlers (auto-reply) and external hooks loaded
//! from `~/.beacon/hooks/`

mod auto_reply;
mod executor;
mod loader;
mod types;

pub use auto_reply::AutoReplyRule;
pub use types::{HookAction, HookEvent, HookResult};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use loader::DiscoveredHook;

/// Hook configuration
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct HooksConfig {
    /// Enable hook system
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Custom hooks directory (default: ~/.beacon/hooks)
    pub path: Option<PathBuf>,
    /// Auto-reply rules
    #[serde(default)]
    pub auto_reply: Vec<AutoReplyRule>,
}

fn default_true() -> bool {
    true
}

/// Hook manager
pub struct HookManager {
    enabled: bool,
    auto_reply: auto_reply::AutoReplyHandler,
    external_hooks: HashMap<HookAction, Vec<Arc<DiscoveredHook>>>,
}

impl HookManager {
    /// Create a new hook manager
    #[must_use]
    pub fn new(config: &HooksConfig, data_dir: &std::path::Path) -> Self {
        if !config.enabled {
            tracing::info!("hooks disabled");
            return Self {
                enabled: false,
                auto_reply: auto_reply::AutoReplyHandler::new(&[]),
                external_hooks: HashMap::new(),
            };
        }

        // Load auto-reply handler
        let auto_reply = auto_reply::AutoReplyHandler::new(&config.auto_reply);

        // Discover external hooks
        let hooks_dir = config
            .path
            .clone()
            .unwrap_or_else(|| data_dir.join("hooks"));

        let discovered = loader::discover_hooks(&hooks_dir);

        // Index hooks by event
        let mut external_hooks: HashMap<HookAction, Vec<Arc<DiscoveredHook>>> = HashMap::new();
        for hook in discovered {
            let hook = Arc::new(hook);
            for event in &hook.events {
                external_hooks
                    .entry(*event)
                    .or_default()
                    .push(Arc::clone(&hook));
            }
        }

        let total_external: usize = external_hooks.values().map(Vec::len).sum();
        tracing::info!(
            auto_reply_rules = config.auto_reply.len(),
            external_hooks = total_external,
            "hook manager initialized"
        );

        Self {
            enabled: true,
            auto_reply,
            external_hooks,
        }
    }

    /// Trigger hooks for an event
    ///
    /// Runs auto-reply first, then external hooks in discovery order
    pub async fn trigger(&self, event: &HookEvent) -> HookResult {
        if !self.enabled {
            return HookResult::default();
        }

        let mut result = HookResult::default();

        // Check auto-reply first
        if let Some(auto_result) = self.auto_reply.handle(event) {
            tracing::debug!(
                action = %event.action,
                has_reply = auto_result.reply.is_some(),
                skip_agent = auto_result.skip_agent,
                "auto-reply triggered"
            );
            result.merge(auto_result);

            // If skip_processing, don't run external hooks
            if result.skip_processing {
                return result;
            }
        }

        // Run external hooks
        let action = match HookAction::from_str(&event.action) {
            Some(a) => a,
            None => return result,
        };

        if let Some(hooks) = self.external_hooks.get(&action) {
            for hook in hooks {
                match executor::execute_hook(&hook.handler_path, event, None).await {
                    Ok(hook_result) => {
                        tracing::debug!(
                            hook = %hook.name,
                            action = %event.action,
                            "hook executed"
                        );
                        result.merge(hook_result);

                        if result.skip_processing {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            hook = %hook.name,
                            error = %e,
                            "hook execution failed"
                        );
                        // Continue with other hooks
                    }
                }
            }
        }

        result
    }

    /// Check if hooks are enabled
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl std::fmt::Debug for HookManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookManager")
            .field("enabled", &self.enabled)
            .field("external_hooks", &self.external_hooks.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_disabled_manager() {
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };

        let manager = HookManager::new(&config, std::path::Path::new("/tmp"));
        assert!(!manager.is_enabled());

        let event = HookEvent {
            action: "message:received".to_string(),
            channel: "test".to_string(),
            channel_id: "ch1".to_string(),
            message_id: "msg1".to_string(),
            sender_id: "user1".to_string(),
            sender_name: "Test".to_string(),
            content: "/help".to_string(),
            thread_id: None,
            session_id: None,
            response: None,
            context: Default::default(),
        };

        let result = manager.trigger(&event).await;
        assert!(result.reply.is_none());
    }

    #[tokio::test]
    async fn test_auto_reply_integration() {
        let config = HooksConfig {
            enabled: true,
            path: None,
            auto_reply: vec![AutoReplyRule {
                pattern: "^/ping$".to_string(),
                reply: "pong".to_string(),
                channels: vec![],
                skip_agent: true,
                case_insensitive: true,
            }],
        };

        let temp_dir = tempfile::TempDir::new().unwrap();
        let manager = HookManager::new(&config, temp_dir.path());

        let event = HookEvent {
            action: "message:received".to_string(),
            channel: "test".to_string(),
            channel_id: "ch1".to_string(),
            message_id: "msg1".to_string(),
            sender_id: "user1".to_string(),
            sender_name: "Test".to_string(),
            content: "/ping".to_string(),
            thread_id: None,
            session_id: None,
            response: None,
            context: Default::default(),
        };

        let result = manager.trigger(&event).await;
        assert_eq!(result.reply, Some("pong".to_string()));
        assert!(result.skip_agent);
    }
}
