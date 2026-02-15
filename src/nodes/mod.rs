//! Node registry for multi-device dispatch
//!
//! Nodes are connected devices that register their capabilities
//! and can receive commands from the gateway

pub mod policy;
pub mod registry;
pub mod types;

pub use policy::{is_command_allowed, platform_defaults};
pub use registry::NodeRegistry;
pub use types::{InvokeRequest, InvokeResult, NodeRegistration, NodeSession};
