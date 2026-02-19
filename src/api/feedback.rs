//! Feedback manager — parks pending `ask_user` / permission requests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// The answer a client sends back for a pending feedback request.
#[derive(Debug, Clone)]
pub enum FeedbackAnswer {
    /// User selected an option or typed a reply.
    Text(String),
    /// User approved (for permission requests).
    Allow,
    /// User approved for the entire session.
    AllowSession,
    /// User denied or dismissed.
    Denied,
    /// Dialog was cancelled (e.g., timeout, disconnect).
    Cancelled,
}

type PendingMap = Mutex<HashMap<Uuid, tokio::sync::oneshot::Sender<FeedbackAnswer>>>;

/// Parks pending feedback requests until the client responds.
#[derive(Default)]
pub struct FeedbackManager {
    pending: Arc<PendingMap>,
}

impl FeedbackManager {
    /// Create a new `FeedbackManager`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending request; returns a receiver that resolves on response.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn register(&self, id: Uuid) -> tokio::sync::oneshot::Receiver<FeedbackAnswer> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        rx
    }

    /// Resolve a pending request with the client's answer.
    ///
    /// Silently ignores unknown IDs (client may respond after timeout).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn respond(&self, id: Uuid, answer: FeedbackAnswer) {
        let tx = self.pending.lock().unwrap().remove(&id);
        if let Some(tx) = tx {
            let _ = tx.send(answer);
        }
    }

    /// Cancel all pending requests (used on disconnect).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn cancel_all(&self) {
        let mut pending = self.pending.lock().unwrap();
        for tx in pending.drain().map(|(_, v)| v) {
            let _ = tx.send(FeedbackAnswer::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_registered_request() {
        let mgr = FeedbackManager::new();
        let id = uuid::Uuid::new_v4();
        let rx = mgr.register(id);
        mgr.respond(id, FeedbackAnswer::Text("yes".to_string()));
        let answer = rx.await.expect("should resolve");
        assert!(matches!(answer, FeedbackAnswer::Text(s) if s == "yes"));
    }

    #[tokio::test]
    async fn unknown_id_is_ignored() {
        let mgr = FeedbackManager::new();
        // responding to an unregistered ID must not panic
        mgr.respond(uuid::Uuid::new_v4(), FeedbackAnswer::Cancelled);
    }
}
