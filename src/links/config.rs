//! Link processing configuration

use serde::{Deserialize, Serialize};

/// Configuration for link processing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkConfig {
    /// Enable link preview extraction
    pub enabled: bool,
    /// Maximum URLs to process per message
    pub max_urls: usize,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_urls: 3,
            timeout_secs: 10,
        }
    }
}
