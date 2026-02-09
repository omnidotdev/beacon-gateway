//! Canvas system for agent-driven visual workspace
//!
//! Provides a canvas abstraction that agents can use to display rich content
//! to connected clients. Content is broadcast via WebSocket to all subscribers.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use crate::Result;

/// Channel capacity for canvas updates
const CHANNEL_CAPACITY: usize = 64;

/// Canvas command from agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasCommand {
    /// Push new content to canvas
    Push { content: CanvasContent },
    /// Clear the canvas
    Clear,
    /// Update specific element by ID
    Update { id: String, content: CanvasContent },
}

/// Canvas content types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasContent {
    /// Markdown text
    Markdown { text: String },
    /// HTML content
    Html { html: String },
    /// Image (base64 or URL)
    Image { src: String, alt: Option<String> },
    /// Code block
    Code { language: String, code: String },
    /// Table data
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Chart/visualization (JSON spec, e.g. Vega-Lite)
    Chart { spec: serde_json::Value },
}

/// Canvas element with ID for updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasElement {
    /// Unique identifier for this element
    pub id: String,
    /// Content of the element
    pub content: CanvasContent,
}

/// Canvas state manager
///
/// Maintains a list of canvas elements and broadcasts updates to subscribers.
pub struct Canvas {
    /// Current canvas elements
    elements: Vec<CanvasElement>,
    /// Broadcast channel for updates
    tx: broadcast::Sender<CanvasCommand>,
    /// Counter for generating element IDs
    next_id: u64,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    /// Create a new empty canvas
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            elements: Vec::new(),
            tx,
            next_id: 0,
        }
    }

    /// Subscribe to canvas updates
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<CanvasCommand> {
        self.tx.subscribe()
    }

    /// Push content to canvas
    ///
    /// Returns the ID of the newly created element
    pub fn push(&mut self, content: CanvasContent) -> String {
        let id = self.generate_id();
        self.elements.push(CanvasElement {
            id: id.clone(),
            content: content.clone(),
        });

        // Broadcast to subscribers (ignore errors if no subscribers)
        let _ = self.tx.send(CanvasCommand::Push { content });

        id
    }

    /// Update an existing element by ID
    ///
    /// Returns true if the element was found and updated
    pub fn update(&mut self, id: &str, content: CanvasContent) -> bool {
        if let Some(element) = self.elements.iter_mut().find(|e| e.id == id) {
            element.content = content.clone();

            // Broadcast update
            let _ = self.tx.send(CanvasCommand::Update {
                id: id.to_string(),
                content,
            });

            true
        } else {
            false
        }
    }

    /// Clear all canvas elements
    pub fn clear(&mut self) {
        self.elements.clear();
        let _ = self.tx.send(CanvasCommand::Clear);
    }

    /// Get current elements
    #[must_use]
    pub fn elements(&self) -> &[CanvasElement] {
        &self.elements
    }

    /// Get element by ID
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CanvasElement> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Remove element by ID
    ///
    /// Returns true if the element was found and removed
    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(pos) = self.elements.iter().position(|e| e.id == id) {
            self.elements.remove(pos);
            true
        } else {
            false
        }
    }

    /// Generate a unique element ID
    fn generate_id(&mut self) -> String {
        let id = format!("canvas-{}", self.next_id);
        self.next_id += 1;
        id
    }
}

/// Canvas tools for agent integration
///
/// Provides async methods that agents can call to manipulate the canvas.
#[derive(Clone)]
pub struct CanvasTools {
    canvas: Arc<Mutex<Canvas>>,
}

impl CanvasTools {
    /// Create new canvas tools wrapping a shared canvas
    #[must_use]
    pub fn new(canvas: Arc<Mutex<Canvas>>) -> Self {
        Self { canvas }
    }

    /// Push content to canvas (agent tool)
    ///
    /// # Errors
    ///
    /// Returns error if the lock cannot be acquired
    pub async fn push(&self, content: CanvasContent) -> Result<String> {
        let mut canvas = self.canvas.lock().await;
        Ok(canvas.push(content))
    }

    /// Update existing element (agent tool)
    ///
    /// # Errors
    ///
    /// Returns error if the element is not found
    pub async fn update(&self, id: &str, content: CanvasContent) -> Result<()> {
        let mut canvas = self.canvas.lock().await;
        if canvas.update(id, content) {
            Ok(())
        } else {
            Err(crate::Error::NotFound(format!("canvas element: {id}")))
        }
    }

    /// Clear canvas (agent tool)
    pub async fn clear(&self) {
        let mut canvas = self.canvas.lock().await;
        canvas.clear();
    }

    /// Get current canvas state
    pub async fn snapshot(&self) -> Vec<CanvasElement> {
        let canvas = self.canvas.lock().await;
        canvas.elements().to_vec()
    }

    /// Subscribe to canvas updates
    pub async fn subscribe(&self) -> broadcast::Receiver<CanvasCommand> {
        let canvas = self.canvas.lock().await;
        canvas.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_push_and_clear() {
        let mut canvas = Canvas::new();

        let id = canvas.push(CanvasContent::Markdown {
            text: "Hello".to_string(),
        });
        assert_eq!(canvas.elements().len(), 1);
        assert!(canvas.get(&id).is_some());

        canvas.clear();
        assert!(canvas.elements().is_empty());
    }

    #[test]
    fn canvas_update_element() {
        let mut canvas = Canvas::new();

        let id = canvas.push(CanvasContent::Markdown {
            text: "Initial".to_string(),
        });

        let updated = canvas.update(
            &id,
            CanvasContent::Markdown {
                text: "Updated".to_string(),
            },
        );
        assert!(updated);

        if let Some(element) = canvas.get(&id) {
            if let CanvasContent::Markdown { text } = &element.content {
                assert_eq!(text, "Updated");
            } else {
                panic!("expected markdown content");
            }
        }
    }

    #[test]
    fn canvas_remove_element() {
        let mut canvas = Canvas::new();

        let id = canvas.push(CanvasContent::Markdown {
            text: "Test".to_string(),
        });
        assert_eq!(canvas.elements().len(), 1);

        assert!(canvas.remove(&id));
        assert!(canvas.elements().is_empty());

        // Removing non-existent should return false
        assert!(!canvas.remove("nonexistent"));
    }

    #[tokio::test]
    async fn canvas_tools_push() {
        let canvas = Arc::new(Mutex::new(Canvas::new()));
        let tools = CanvasTools::new(canvas.clone());

        let id = tools
            .push(CanvasContent::Code {
                language: "rust".to_string(),
                code: "fn main() {}".to_string(),
            })
            .await
            .unwrap();

        assert!(!id.is_empty());

        let snapshot = tools.snapshot().await;
        assert_eq!(snapshot.len(), 1);
    }

    #[tokio::test]
    async fn canvas_broadcast_subscription() {
        let canvas = Arc::new(Mutex::new(Canvas::new()));
        let tools = CanvasTools::new(canvas);

        let mut rx = tools.subscribe().await;

        tools
            .push(CanvasContent::Markdown {
                text: "Broadcast test".to_string(),
            })
            .await
            .unwrap();

        let cmd = rx.recv().await.unwrap();
        assert!(matches!(cmd, CanvasCommand::Push { .. }));
    }
}
