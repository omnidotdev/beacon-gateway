//! Cron tools for scheduling recurring tasks via Vortex
//!
//! Provides agent-accessible tools for scheduling, listing, and canceling
//! recurring tasks through the Vortex scheduling service

use serde::{Deserialize, Serialize};

use crate::integrations::{ScheduleRequest, VortexClient};
use crate::Result;

/// Tools for managing scheduled tasks via Vortex
#[derive(Debug, Clone)]
pub struct CronTools {
    /// Vortex client for API calls
    vortex: VortexClient,
    /// Base URL for callbacks (e.g., `<http://localhost:8080/webhooks/vortex>`)
    callback_base_url: String,
}

impl CronTools {
    /// Create a new `CronTools` instance
    ///
    /// # Arguments
    ///
    /// * `vortex` - Configured Vortex client
    /// * `callback_base_url` - Base URL where Vortex will send callbacks
    #[must_use]
    pub fn new(vortex: VortexClient, callback_base_url: impl Into<String>) -> Self {
        Self {
            vortex,
            callback_base_url: callback_base_url.into(),
        }
    }

    /// Schedule a recurring task
    ///
    /// # Arguments
    ///
    /// * `cron` - Cron expression (e.g., "0 9 * * MON" for 9 AM every Monday)
    /// * `action` - Action type to trigger (e.g., `remind`, `check_in`)
    /// * `payload` - Arbitrary JSON data to include in callback
    ///
    /// # Returns
    ///
    /// The schedule ID on success
    ///
    /// # Errors
    ///
    /// Returns an error if the Vortex API call fails
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let tools = CronTools::new(vortex, "http://localhost:8080/webhooks/vortex");
    /// let id = tools.schedule(
    ///     "0 9 * * MON",
    ///     "remind",
    ///     serde_json::json!({ "message": "Weekly standup" }),
    /// ).await?;
    /// ```
    pub async fn schedule(
        &self,
        cron: &str,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<String> {
        let request = ScheduleRequest {
            cron: cron.to_string(),
            callback_url: self.callback_base_url.clone(),
            action: action.to_string(),
            payload,
            description: None,
            timezone: None,
        };

        let schedule = self.vortex.schedule(&request).await?;
        Ok(schedule.id)
    }

    /// Schedule a recurring task with additional options
    ///
    /// # Arguments
    ///
    /// * `params` - Full schedule parameters
    ///
    /// # Returns
    ///
    /// The schedule ID on success
    ///
    /// # Errors
    ///
    /// Returns an error if the Vortex API call fails
    pub async fn schedule_with_options(&self, params: ScheduleParams) -> Result<String> {
        let request = ScheduleRequest {
            cron: params.cron,
            callback_url: self.callback_base_url.clone(),
            action: params.action,
            payload: params.payload,
            description: params.description,
            timezone: params.timezone,
        };

        let schedule = self.vortex.schedule(&request).await?;
        Ok(schedule.id)
    }

    /// List all scheduled tasks
    ///
    /// # Errors
    ///
    /// Returns an error if the Vortex API call fails
    pub async fn list(&self) -> Result<Vec<ScheduleInfo>> {
        let schedules = self.vortex.list_schedules().await?;

        let infos = schedules
            .into_iter()
            .map(|s| ScheduleInfo {
                id: s.id,
                cron: s.cron,
                action: s.action,
                next_run: s.next_run.map(|dt| dt.to_rfc3339()),
                description: s.description,
                active: s.active,
            })
            .collect();

        Ok(infos)
    }

    /// Cancel a scheduled task
    ///
    /// # Arguments
    ///
    /// * `schedule_id` - ID of the schedule to cancel
    ///
    /// # Errors
    ///
    /// Returns an error if the Vortex API call fails
    pub async fn cancel(&self, schedule_id: &str) -> Result<()> {
        self.vortex.cancel_schedule(schedule_id).await
    }

    /// Get details of a specific schedule
    ///
    /// # Arguments
    ///
    /// * `schedule_id` - ID of the schedule to retrieve
    ///
    /// # Errors
    ///
    /// Returns an error if the Vortex API call fails or schedule not found
    pub async fn get(&self, schedule_id: &str) -> Result<ScheduleInfo> {
        let schedule = self.vortex.get_schedule(schedule_id).await?;

        Ok(ScheduleInfo {
            id: schedule.id,
            cron: schedule.cron,
            action: schedule.action,
            next_run: schedule.next_run.map(|dt| dt.to_rfc3339()),
            description: schedule.description,
            active: schedule.active,
        })
    }
}

/// Parameters for scheduling a task with full options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleParams {
    /// Cron expression (e.g., "0 9 * * MON")
    pub cron: String,
    /// Action type (e.g., `remind`, `check_in`)
    pub action: String,
    /// Arbitrary payload data
    pub payload: serde_json::Value,
    /// Optional human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional timezone (defaults to UTC)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Summary information about a scheduled task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleInfo {
    /// Unique identifier
    pub id: String,
    /// Cron expression
    pub cron: String,
    /// Action type
    pub action: String,
    /// Next scheduled run time (ISO 8601 string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the schedule is active
    pub active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_info_serialization() {
        let info = ScheduleInfo {
            id: "sched_123".to_string(),
            cron: "0 9 * * MON".to_string(),
            action: "remind".to_string(),
            next_run: Some("2024-01-08T09:00:00Z".to_string()),
            description: Some("Weekly reminder".to_string()),
            active: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("sched_123"));
        assert!(json.contains("0 9 * * MON"));
    }

    #[test]
    fn test_schedule_params_deserialization() {
        let json = r#"{
            "cron": "0 9 * * MON",
            "action": "remind",
            "payload": { "message": "Test" }
        }"#;

        let params: ScheduleParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.cron, "0 9 * * MON");
        assert_eq!(params.action, "remind");
        assert!(params.description.is_none());
    }
}
