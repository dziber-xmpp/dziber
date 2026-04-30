use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub body: String,
    pub timestamp: DateTime<Utc>,
    pub status: MessageStatus,
    pub direction: Direction,
}

impl Message {
    pub fn new(from: String, body: String, direction: Direction) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from,
            body,
            timestamp: Utc::now(),
            status: MessageStatus::Pending,
            direction,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStatus {
    Pending,
    Sent,
    Delivered,
    Received,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Incoming,
    Outgoing,
}
