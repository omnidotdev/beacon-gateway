//! Node registry for tracking connected devices

use std::collections::HashMap;

use tokio::sync::oneshot;
use uuid::Uuid;

use super::types::{InvokeResult, NodeRegistration, NodeSession};

/// Registry of connected nodes
#[derive(Debug)]
pub struct NodeRegistry {
    nodes: HashMap<String, NodeSession>,
    /// Pending invocation responses keyed by correlation ID
    pending: HashMap<String, oneshot::Sender<InvokeResult>>,
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeRegistry {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Register a node and return its assigned node ID
    pub fn register(&mut self, registration: NodeRegistration) -> String {
        let node_id = format!("node_{}", Uuid::new_v4());
        let session = NodeSession {
            node_id: node_id.clone(),
            device_id: registration.device_id,
            display_name: registration.display_name,
            platform: registration.platform,
            device_family: registration.device_family,
            caps: registration.caps,
            commands: registration.commands,
            connected_at: chrono::Utc::now(),
        };
        self.nodes.insert(node_id.clone(), session);
        node_id
    }

    /// Unregister a node, cleaning up any pending invocations
    pub fn unregister(&mut self, node_id: &str) -> Option<NodeSession> {
        self.nodes.remove(node_id)
    }

    /// Get a node by ID
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<&NodeSession> {
        self.nodes.get(node_id)
    }

    /// List all connected nodes
    #[must_use]
    pub fn list(&self) -> Vec<&NodeSession> {
        self.nodes.values().collect()
    }

    /// Find a node that has the given capability
    #[must_use]
    pub fn find_by_cap(&self, cap: &str) -> Option<&NodeSession> {
        self.nodes.values().find(|n| n.caps.iter().any(|c| c == cap))
    }

    /// Find a node that supports the given command
    #[must_use]
    pub fn find_by_command(&self, command: &str) -> Option<&NodeSession> {
        self.nodes
            .values()
            .find(|n| n.commands.iter().any(|c| c == command))
    }

    /// Prepare an invocation, returning (correlation_id, receiver)
    ///
    /// The caller should send the request to the node and await the receiver
    ///
    /// # Errors
    ///
    /// Returns error if the node is not found
    pub fn prepare_invoke(
        &mut self,
        node_id: &str,
    ) -> anyhow::Result<(String, oneshot::Receiver<InvokeResult>)> {
        if !self.nodes.contains_key(node_id) {
            anyhow::bail!("node '{node_id}' not found");
        }

        let correlation_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(correlation_id.clone(), tx);

        Ok((correlation_id, rx))
    }

    /// Handle a response from a node for a pending invocation
    ///
    /// Returns true if the correlation ID was found and resolved
    pub fn handle_response(&mut self, correlation_id: &str, result: InvokeResult) -> bool {
        if let Some(tx) = self.pending.remove(correlation_id) {
            tx.send(result).is_ok()
        } else {
            false
        }
    }

    /// Number of connected nodes
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the registry is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registration() -> NodeRegistration {
        NodeRegistration {
            device_id: "device_123".to_string(),
            display_name: Some("My MacBook".to_string()),
            platform: "darwin".to_string(),
            device_family: Some("laptop".to_string()),
            caps: vec!["audio".to_string(), "display".to_string()],
            commands: vec!["system.run".to_string(), "device.info".to_string()],
        }
    }

    #[test]
    fn register_and_get() {
        let mut registry = NodeRegistry::new();
        let node_id = registry.register(sample_registration());

        let node = registry.get(&node_id).unwrap();
        assert_eq!(node.device_id, "device_123");
        assert_eq!(node.platform, "darwin");
    }

    #[test]
    fn unregister_removes_node() {
        let mut registry = NodeRegistry::new();
        let node_id = registry.register(sample_registration());
        assert_eq!(registry.len(), 1);

        let removed = registry.unregister(&node_id);
        assert!(removed.is_some());
        assert!(registry.is_empty());
    }

    #[test]
    fn find_by_cap() {
        let mut registry = NodeRegistry::new();
        registry.register(sample_registration());

        assert!(registry.find_by_cap("audio").is_some());
        assert!(registry.find_by_cap("camera").is_none());
    }

    #[test]
    fn find_by_command() {
        let mut registry = NodeRegistry::new();
        registry.register(sample_registration());

        assert!(registry.find_by_command("system.run").is_some());
        assert!(registry.find_by_command("browser.proxy").is_none());
    }

    #[test]
    fn prepare_invoke_unknown_node() {
        let mut registry = NodeRegistry::new();
        assert!(registry.prepare_invoke("nonexistent").is_err());
    }

    #[test]
    fn invoke_round_trip() {
        let mut registry = NodeRegistry::new();
        let node_id = registry.register(sample_registration());

        let (corr_id, mut rx) = registry.prepare_invoke(&node_id).unwrap();

        let result = InvokeResult {
            ok: true,
            payload: Some(serde_json::json!({"status": "done"})),
            error: None,
        };

        assert!(registry.handle_response(&corr_id, result));

        let received = rx.try_recv().unwrap();
        assert!(received.ok);
    }
}
