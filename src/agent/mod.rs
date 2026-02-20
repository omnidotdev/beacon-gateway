//! Agentic runner — shared logic for proactive and WebSocket-driven turns

pub mod runner;

pub use runner::{AgentNotifyEvent, AgentRunConfig, run_agent_turn};
