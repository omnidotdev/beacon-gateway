//! iMessage channel adapter
//!
//! Uses the `imsg` CLI tool for macOS Messages.app integration.
//! Requires: brew install steipete/tap/imsg
//! See: <https://github.com/steipete/imsg>

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Attachment, AttachmentKind, Channel, IncomingMessage, OutgoingMessage};
use crate::{Error, Result};

/// iMessage channel adapter
///
/// Spawns `imsg rpc` and communicates via JSON-RPC over stdin/stdout
pub struct IMessageChannel {
    /// Path to imsg CLI (defaults to "imsg")
    cli_path: String,
    /// Optional database path override
    db_path: Option<String>,
    /// Region for phone number normalization (defaults to "US")
    region: String,
    /// Service preference: "iMessage", "SMS", or "auto"
    service: String,
    /// RPC client state
    state: Arc<Mutex<IMessageState>>,
    /// Connection status
    connected: AtomicBool,
    /// Next request ID
    next_id: AtomicU64,
    /// Message sender for incoming messages
    message_tx: Option<mpsc::Sender<IncomingMessage>>,
}

#[derive(Default)]
struct IMessageState {
    child: Option<Child>,
    pending: HashMap<u64, tokio::sync::oneshot::Sender<RpcResponse>>,
}

/// JSON-RPC request
#[derive(Debug, Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: serde_json::Value,
}

/// JSON-RPC response
#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<RpcError>,
    method: Option<String>,
    params: Option<serde_json::Value>,
}

/// JSON-RPC error
#[derive(Debug, Deserialize)]
struct RpcError {
    code: Option<i32>,
    message: Option<String>,
    data: Option<serde_json::Value>,
}

/// iMessage chat info
#[derive(Debug, Clone, Deserialize)]
pub struct IMessageChat {
    pub id: i64,
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub service: Option<String>,
    pub last_message_at: Option<String>,
    pub is_group: Option<bool>,
    pub participants: Option<Vec<String>>,
}

/// iMessage message info
#[derive(Debug, Clone, Deserialize)]
pub struct IMessageMessage {
    pub id: i64,
    pub chat_id: i64,
    pub guid: Option<String>,
    pub sender: Option<String>,
    pub is_from_me: bool,
    pub text: Option<String>,
    pub created_at: Option<String>,
    pub attachments: Option<Vec<IMessageAttachment>>,
}

/// iMessage attachment
#[derive(Debug, Clone, Deserialize)]
pub struct IMessageAttachment {
    pub path: Option<String>,
    pub mime: Option<String>,
    pub name: Option<String>,
}

/// Result from sending a message via RPC
#[derive(Debug, Deserialize)]
struct SendResult {
    message_id: Option<String>,
    ok: Option<String>,
}

impl IMessageChannel {
    /// Create a new iMessage channel adapter
    ///
    /// # Arguments
    ///
    /// * `cli_path` - Path to imsg CLI (defaults to "imsg")
    /// * `db_path` - Optional database path override
    /// * `region` - Region for phone normalization (defaults to "US")
    /// * `service` - Service preference: "iMessage", "SMS", or "auto" (defaults to "auto")
    #[must_use]
    pub fn new(
        cli_path: Option<String>,
        db_path: Option<String>,
        region: Option<String>,
        service: Option<String>,
    ) -> Self {
        Self {
            cli_path: cli_path.unwrap_or_else(|| "imsg".to_string()),
            db_path,
            region: region.unwrap_or_else(|| "US".to_string()),
            service: service.unwrap_or_else(|| "auto".to_string()),
            state: Arc::new(Mutex::new(IMessageState::default())),
            connected: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            message_tx: None,
        }
    }

    /// Create with a message receiver
    ///
    /// Returns the channel and a receiver for incoming messages
    #[must_use]
    pub fn with_receiver(
        cli_path: Option<String>,
        db_path: Option<String>,
        region: Option<String>,
        service: Option<String>,
    ) -> (Self, mpsc::Receiver<IncomingMessage>) {
        let (tx, rx) = mpsc::channel(100);
        let channel = Self {
            cli_path: cli_path.unwrap_or_else(|| "imsg".to_string()),
            db_path,
            region: region.unwrap_or_else(|| "US".to_string()),
            service: service.unwrap_or_else(|| "auto".to_string()),
            state: Arc::new(Mutex::new(IMessageState::default())),
            connected: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            message_tx: Some(tx),
        };
        (channel, rx)
    }

    /// Send an RPC request and wait for response
    async fn request<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let request_line =
            serde_json::to_string(&request).map_err(|e| Error::Channel(format!("JSON error: {e}")))?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        // Write request to stdin
        {
            let mut state = self
                .state
                .lock()
                .map_err(|e| Error::Channel(format!("Lock error: {e}")))?;

            let child = state
                .child
                .as_mut()
                .ok_or_else(|| Error::Channel("imsg rpc not running".to_string()))?;

            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| Error::Channel("No stdin".to_string()))?;

            writeln!(stdin, "{request_line}")
                .map_err(|e| Error::Channel(format!("Write error: {e}")))?;

            state.pending.insert(id, tx);
        }

        // Wait for response
        let response = rx
            .await
            .map_err(|_| Error::Channel("RPC response channel closed".to_string()))?;

        if let Some(error) = response.error {
            let msg = error.message.unwrap_or_else(|| "Unknown error".to_string());
            return Err(Error::Channel(format!("imsg RPC error: {msg}")));
        }

        let result = response
            .result
            .ok_or_else(|| Error::Channel("Empty result".to_string()))?;

        serde_json::from_value(result).map_err(|e| Error::Channel(format!("Parse error: {e}")))
    }

    /// List all chats
    ///
    /// # Errors
    ///
    /// Returns error if RPC fails
    pub async fn list_chats(&self) -> Result<Vec<IMessageChat>> {
        self.request("chats", serde_json::json!({})).await
    }

    /// Get message history for a chat
    ///
    /// # Errors
    ///
    /// Returns error if RPC fails
    pub async fn history(&self, chat_id: i64, limit: Option<u32>) -> Result<Vec<IMessageMessage>> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "limit": limit.unwrap_or(50)
        });
        self.request("history", params).await
    }

    /// Send a message
    ///
    /// # Errors
    ///
    /// Returns error if send fails
    pub async fn send_message(&self, to: &str, text: &str) -> Result<String> {
        let params = serde_json::json!({
            "to": to,
            "text": text,
            "service": self.service,
            "region": self.region
        });

        let result: SendResult = self.request("send", params).await?;
        Ok(result.message_id.or(result.ok).unwrap_or_else(|| "sent".to_string()))
    }

    /// Send a message to a chat by ID
    ///
    /// # Errors
    ///
    /// Returns error if send fails
    pub async fn send_to_chat(&self, chat_id: i64, text: &str) -> Result<String> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "service": self.service
        });

        let result: SendResult = self.request("send", params).await?;
        Ok(result.message_id.or(result.ok).unwrap_or_else(|| "sent".to_string()))
    }

    /// Start watching for new messages
    ///
    /// Spawns a background thread that reads RPC notifications from imsg and
    /// forwards them to the message sender (if configured via `with_receiver`)
    fn start_watching(&self) {
        let tx = match &self.message_tx {
            Some(tx) => tx.clone(),
            None => return,
        };

        // Start watch in background
        let state = Arc::clone(&self.state);
        let _handle = std::thread::spawn(move || {
            // Read notifications from stdout
            let Ok(mut state_guard) = state.lock() else {
                return;
            };

            let Some(child) = state_guard.child.as_mut() else {
                return;
            };

            let Some(stdout) = child.stdout.take() else {
                return;
            };

            drop(state_guard);

            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(std::result::Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }

                if let Ok(response) = serde_json::from_str::<RpcResponse>(&line) {
                    // Check if this is a notification (no id, has method)
                    if response.id.is_none() && response.method.as_deref() == Some("message") {
                        if let Some(params) = response.params {
                            if let Ok(msg) = serde_json::from_value::<IMessageMessage>(params) {
                                // Extract attachments from local filesystem
                                let attachments = msg
                                    .attachments
                                    .as_ref()
                                    .map(|atts| {
                                        atts.iter()
                                            .map(|att| {
                                                let mime = att.mime.clone().unwrap_or_else(|| "application/octet-stream".to_string());
                                                let filename = att.name.clone();

                                                // Try to read file data from path
                                                if let Some(path) = &att.path {
                                                    if let Ok(data) = std::fs::read(path) {
                                                        return Attachment::from_data(data, mime, filename);
                                                    }
                                                }

                                                // If no path or read failed, still include metadata
                                                Attachment {
                                                    kind: AttachmentKind::from_mime(&mime),
                                                    url: None,
                                                    data: None,
                                                    mime_type: mime,
                                                    filename,
                                                }
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                let incoming = IncomingMessage {
                                    id: msg.id.to_string(),
                                    channel_id: msg.chat_id.to_string(),
                                    sender_id: msg.sender.clone().unwrap_or_default(),
                                    sender_name: msg.sender.unwrap_or_default(),
                                    content: msg.text.unwrap_or_default(),
                                    is_dm: true,
                                    reply_to: None,
                                    attachments,
                                };
                                let _ = tx.blocking_send(incoming);
                            }
                        }
                    } else if let Some(id) = response.id {
                        // This is a response to a pending request
                        if let Ok(mut guard) = state.lock() {
                            if let Some(sender) = guard.pending.remove(&id) {
                                let _ = sender.send(response);
                            }
                        }
                    }
                }
            }
        });
    }
}

#[async_trait]
impl Channel for IMessageChannel {
    fn name(&self) -> &'static str {
        "imessage"
    }

    async fn connect(&mut self) -> Result<()> {
        let mut args = vec!["rpc".to_string()];

        if let Some(ref db_path) = self.db_path {
            args.push("--db".to_string());
            args.push(db_path.clone());
        }

        let child = Command::new(&self.cli_path)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Channel(format!(
                    "Failed to start imsg rpc: {e}. Install with: brew install steipete/tap/imsg"
                ))
            })?;

        {
            let mut state = self
                .state
                .lock()
                .map_err(|e| Error::Channel(format!("Lock error: {e}")))?;
            state.child = Some(child);
        }

        self.connected.store(true, Ordering::SeqCst);
        tracing::info!("iMessage channel connected via imsg rpc");

        // Start watching for incoming messages if configured
        self.start_watching();

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|e| Error::Channel(format!("Lock error: {e}")))?;

            if let Some(mut child) = state.child.take() {
                // Close stdin to signal EOF
                drop(child.stdin.take());
                // Wait briefly then kill if needed
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = child.kill();
            }
        }

        self.connected.store(false, Ordering::SeqCst);
        tracing::info!("iMessage channel disconnected");

        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<()> {
        // Try to parse channel_id as chat_id first
        if let Ok(chat_id) = message.channel_id.parse::<i64>() {
            self.send_to_chat(chat_id, &message.content).await?;
        } else {
            // Treat as phone number or email
            self.send_message(&message.channel_id, &message.content).await?;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

impl Drop for IMessageChannel {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(mut child) = state.child.take() {
                let _ = child.kill();
            }
        }
    }
}
