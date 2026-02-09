//! Vortex scheduling service integration
//!
//! Client for interacting with the Vortex scheduling service to create,
//! list, and cancel scheduled workflows

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Client for the Vortex scheduling service
#[derive(Debug, Clone)]
pub struct VortexClient {
    /// HTTP client
    client: Client,
    /// Base URL for the Vortex API
    base_url: String,
    /// Optional API key for authentication
    api_key: Option<String>,
}

impl VortexClient {
    /// Create a new Vortex client
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL for the Vortex API (e.g., <https://vortex.omni.dev>)
    /// * `api_key` - Optional API key for authentication
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            api_key,
        }
    }

    /// Build the authorization header value
    fn auth_header(&self) -> Option<String> {
        self.api_key.as_ref().map(|key| format!("Bearer {key}"))
    }

    /// Schedule a new workflow
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response is invalid
    pub async fn schedule(&self, request: &ScheduleRequest) -> Result<Schedule> {
        let url = format!("{}/schedules", self.base_url);

        let mut req = self.client.post(&url).json(request);

        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Vortex API error: {status} - {body}"
            )));
        }

        let schedule = response.json().await?;
        Ok(schedule)
    }

    /// List all schedules
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response is invalid
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        let url = format!("{}/schedules", self.base_url);

        let mut req = self.client.get(&url);

        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Vortex API error: {status} - {body}"
            )));
        }

        let schedules = response.json().await?;
        Ok(schedules)
    }

    /// Cancel a schedule
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails
    pub async fn cancel_schedule(&self, id: &str) -> Result<()> {
        let url = format!("{}/schedules/{id}", self.base_url);

        let mut req = self.client.delete(&url);

        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Vortex API error: {status} - {body}"
            )));
        }

        Ok(())
    }

    /// Get a specific schedule by ID
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or schedule not found
    pub async fn get_schedule(&self, id: &str) -> Result<Schedule> {
        let url = format!("{}/schedules/{id}", self.base_url);

        let mut req = self.client.get(&url);

        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Channel(format!(
                "Vortex API error: {status} - {body}"
            )));
        }

        let schedule = response.json().await?;
        Ok(schedule)
    }
}

/// Request to create a new schedule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRequest {
    /// Cron expression (e.g., "0 9 * * MON")
    pub cron: String,
    /// Callback URL to invoke when the schedule triggers
    pub callback_url: String,
    /// Action type (e.g., `remind`, `check_in`)
    pub action: String,
    /// Arbitrary payload to include in callback
    pub payload: serde_json::Value,
    /// Optional human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional timezone (defaults to UTC)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// A scheduled workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique identifier
    pub id: String,
    /// Cron expression
    pub cron: String,
    /// Callback URL
    pub callback_url: String,
    /// Action type
    pub action: String,
    /// Payload data
    pub payload: serde_json::Value,
    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Timezone
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Next scheduled run time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<DateTime<Utc>>,
    /// Last run time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Whether the schedule is active
    #[serde(default = "default_active")]
    pub active: bool,
}

const fn default_active() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_request_serialization() {
        let request = ScheduleRequest {
            cron: "0 9 * * MON".to_string(),
            callback_url: "http://localhost:8080/webhooks/vortex".to_string(),
            action: "remind".to_string(),
            payload: serde_json::json!({ "message": "Weekly standup" }),
            description: Some("Weekly standup reminder".to_string()),
            timezone: Some("America/New_York".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("0 9 * * MON"));
        assert!(json.contains("remind"));
    }

    #[test]
    fn test_schedule_deserialization() {
        let json = r#"{
            "id": "sched_123",
            "cron": "0 9 * * MON",
            "callback_url": "http://localhost:8080/webhooks/vortex",
            "action": "remind",
            "payload": { "message": "Test" },
            "created_at": "2024-01-01T00:00:00Z"
        }"#;

        let schedule: Schedule = serde_json::from_str(json).unwrap();
        assert_eq!(schedule.id, "sched_123");
        assert_eq!(schedule.cron, "0 9 * * MON");
        assert!(schedule.active);
    }
}
