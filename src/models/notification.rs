use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub notification_type: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub title: String,
    pub message: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub deep_link: Option<String>,
    pub priority: Option<String>,
    pub deliver_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl Notification {
    /// Check if this is a high-priority notification that should always push
    pub fn is_high_priority(&self) -> bool {
        matches!(
            self.priority.as_deref(),
            Some("high") | Some("critical")
        )
    }
}

/// Message sent to client via WebSocket
#[derive(Debug, Serialize)]
pub struct SyncNotifyMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub count: usize,
}

impl SyncNotifyMessage {
    pub fn new(count: usize) -> Self {
        Self {
            msg_type: "sync_notify",
            count,
        }
    }
}

/// WebSocket connected message
#[derive(Debug, Serialize)]
pub struct ConnectedMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub user_id: String,
    pub connection_count: u32,
    pub unread_count: u32,
    pub supports_replay: bool,
}

impl ConnectedMessage {
    pub fn new(user_id: Uuid) -> Self {
        Self {
            msg_type: "connected",
            user_id: user_id.to_string(),
            connection_count: 1,
            unread_count: 0,  // TODO: fetch from DB
            supports_replay: false,  // Not yet implemented
        }
    }
}

/// Pong response
#[derive(Debug, Serialize)]
pub struct PongMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
}

impl Default for PongMessage {
    fn default() -> Self {
        Self { msg_type: "pong" }
    }
}

/// Client message types
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Ping,
    SyncComplete {
        notification_ids: Vec<Uuid>,
    },
}
