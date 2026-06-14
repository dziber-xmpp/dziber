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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_new() {
        let message = Message::new(
            "contact@example.com".to_string(),
            "Hello".to_string(),
            Direction::Incoming,
        );
        assert_eq!(message.from, "contact@example.com");
        assert_eq!(message.body, "Hello");
        assert_eq!(message.direction, Direction::Incoming);
        assert_eq!(message.status, MessageStatus::Pending);
        assert!(!message.id.is_empty());
    }

    #[test]
    fn message_serde_roundtrip() {
        let message = Message {
            id: "m1".to_string(),
            from: "contact@example.com".to_string(),
            body: "Hello".to_string(),
            timestamp: Utc::now(),
            status: MessageStatus::Delivered,
            direction: Direction::Outgoing,
        };
        let json = serde_json::to_string(&message).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn message_status_variants_serde_roundtrip() {
        for status in [
            MessageStatus::Pending,
            MessageStatus::Sent,
            MessageStatus::Delivered,
            MessageStatus::Received,
            MessageStatus::Error,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: MessageStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, decoded);
        }
    }

    #[test]
    fn direction_variants_serde_roundtrip() {
        for direction in [Direction::Incoming, Direction::Outgoing] {
            let json = serde_json::to_string(&direction).unwrap();
            let decoded: Direction = serde_json::from_str(&json).unwrap();
            assert_eq!(direction, decoded);
        }
    }
}
