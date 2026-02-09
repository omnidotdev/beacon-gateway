//! Webhook endpoints for channel integrations

use std::sync::Arc;

use axum::{routing::post, Router};

use super::ApiState;

pub mod google_chat;
pub mod teams;
pub mod telegram;
pub mod vortex;

/// Build webhooks router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/telegram", post(telegram::handle_update))
        .route("/teams", post(teams::handle_activity))
        .route("/google-chat", post(google_chat::handle_event))
        .route("/vortex", post(vortex::handle_vortex_callback))
        .with_state(state)
}
