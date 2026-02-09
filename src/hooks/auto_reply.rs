//! Built-in auto-reply hook

use regex::Regex;
use serde::Deserialize;

use super::types::{HookAction, HookEvent, HookResult};

/// Auto-reply rule configuration
#[derive(Debug, Clone, Deserialize)]
pub struct AutoReplyRule {
    /// Regex pattern to match against message content
    pub pattern: String,
    /// Reply text to send
    pub reply: String,
    /// Channels this rule applies to (all if empty)
    #[serde(default)]
    pub channels: Vec<String>,
    /// Skip agent call after replying
    #[serde(default)]
    pub skip_agent: bool,
    /// Case insensitive matching
    #[serde(default = "default_true")]
    pub case_insensitive: bool,
}

fn default_true() -> bool {
    true
}

/// Compiled auto-reply rule
pub struct CompiledRule {
    pattern: Regex,
    reply: String,
    channels: Vec<String>,
    skip_agent: bool,
}

impl CompiledRule {
    /// Compile a rule from configuration
    ///
    /// # Errors
    ///
    /// Returns error if regex pattern is invalid
    pub fn compile(rule: &AutoReplyRule) -> Result<Self, regex::Error> {
        let pattern = if rule.case_insensitive {
            Regex::new(&format!("(?i){}", rule.pattern))?
        } else {
            Regex::new(&rule.pattern)?
        };

        Ok(Self {
            pattern,
            reply: rule.reply.clone(),
            channels: rule.channels.clone(),
            skip_agent: rule.skip_agent,
        })
    }

    /// Check if rule matches the event
    fn matches(&self, event: &HookEvent) -> bool {
        // Check channel filter
        if !self.channels.is_empty()
            && !self.channels.iter().any(|c| c == &event.channel)
        {
            return false;
        }

        // Check pattern
        self.pattern.is_match(&event.content)
    }

    /// Apply rule to event and return result
    fn apply(&self, event: &HookEvent) -> HookResult {
        // Support simple template substitution
        let reply = self.expand_reply(event);

        HookResult {
            skip_processing: false,
            skip_agent: self.skip_agent,
            reply: Some(reply),
            modified_response: None,
            messages: vec![],
        }
    }

    /// Expand template variables in reply
    fn expand_reply(&self, event: &HookEvent) -> String {
        self.reply
            .replace("{{sender}}", &event.sender_name)
            .replace("{{channel}}", &event.channel)
            .replace("{{content}}", &event.content)
    }
}

/// Auto-reply handler
pub struct AutoReplyHandler {
    rules: Vec<CompiledRule>,
}

impl AutoReplyHandler {
    /// Create handler from configuration
    ///
    /// Invalid rules are logged and skipped
    #[must_use]
    pub fn new(rules: &[AutoReplyRule]) -> Self {
        let compiled: Vec<_> = rules
            .iter()
            .filter_map(|r| {
                match CompiledRule::compile(r) {
                    Ok(compiled) => Some(compiled),
                    Err(e) => {
                        tracing::warn!(
                            pattern = %r.pattern,
                            error = %e,
                            "invalid auto-reply pattern, skipping"
                        );
                        None
                    }
                }
            })
            .collect();

        tracing::info!(count = compiled.len(), "loaded auto-reply rules");

        Self { rules: compiled }
    }

    /// Check if handler has any rules
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Handle an event
    ///
    /// Only processes `message:received` and `message:before_agent` events
    pub fn handle(&self, event: &HookEvent) -> Option<HookResult> {
        // Only handle early message events
        let action = HookAction::from_str(&event.action)?;
        if !matches!(action, HookAction::MessageReceived | HookAction::BeforeAgent) {
            return None;
        }

        // Find first matching rule
        for rule in &self.rules {
            if rule.matches(event) {
                tracing::debug!(
                    channel = %event.channel,
                    sender = %event.sender_name,
                    "auto-reply rule matched"
                );
                return Some(rule.apply(event));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(channel: &str, content: &str) -> HookEvent {
        HookEvent {
            action: "message:received".to_string(),
            channel: channel.to_string(),
            channel_id: "ch1".to_string(),
            message_id: "msg1".to_string(),
            sender_id: "user1".to_string(),
            sender_name: "Alice".to_string(),
            content: content.to_string(),
            thread_id: None,
            session_id: None,
            response: None,
            context: Default::default(),
        }
    }

    #[test]
    fn test_simple_match() {
        let rules = vec![AutoReplyRule {
            pattern: "^/help$".to_string(),
            reply: "Available commands: /help, /status".to_string(),
            channels: vec![],
            skip_agent: true,
            case_insensitive: true,
        }];

        let handler = AutoReplyHandler::new(&rules);
        let event = make_event("discord", "/help");
        let result = handler.handle(&event).unwrap();

        assert!(result.skip_agent);
        assert_eq!(result.reply.unwrap(), "Available commands: /help, /status");
    }

    #[test]
    fn test_channel_filter() {
        let rules = vec![AutoReplyRule {
            pattern: "hello".to_string(),
            reply: "Hi!".to_string(),
            channels: vec!["slack".to_string()],
            skip_agent: false,
            case_insensitive: true,
        }];

        let handler = AutoReplyHandler::new(&rules);

        // Should not match discord
        let event = make_event("discord", "hello there");
        assert!(handler.handle(&event).is_none());

        // Should match slack
        let event = make_event("slack", "hello there");
        assert!(handler.handle(&event).is_some());
    }

    #[test]
    fn test_template_expansion() {
        let rules = vec![AutoReplyRule {
            pattern: "^hi$".to_string(),
            reply: "Hello {{sender}}!".to_string(),
            channels: vec![],
            skip_agent: false,
            case_insensitive: true,
        }];

        let handler = AutoReplyHandler::new(&rules);
        let event = make_event("discord", "hi");
        let result = handler.handle(&event).unwrap();

        assert_eq!(result.reply.unwrap(), "Hello Alice!");
    }

    #[test]
    fn test_case_insensitive() {
        let rules = vec![AutoReplyRule {
            pattern: "help".to_string(),
            reply: "Need help?".to_string(),
            channels: vec![],
            skip_agent: false,
            case_insensitive: true,
        }];

        let handler = AutoReplyHandler::new(&rules);

        let event = make_event("discord", "HELP");
        assert!(handler.handle(&event).is_some());

        let event = make_event("discord", "Help me");
        assert!(handler.handle(&event).is_some());
    }

    #[test]
    fn test_no_match() {
        let rules = vec![AutoReplyRule {
            pattern: "^/help$".to_string(),
            reply: "Help".to_string(),
            channels: vec![],
            skip_agent: false,
            case_insensitive: true,
        }];

        let handler = AutoReplyHandler::new(&rules);
        let event = make_event("discord", "something else");
        assert!(handler.handle(&event).is_none());
    }
}
