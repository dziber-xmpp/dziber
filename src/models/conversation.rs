use serde::{Deserialize, Serialize};

use super::message::Message;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub contact_jid: String,
    pub account_jid: String,
    pub name: Option<String>,
    pub messages: Vec<Message>,
    pub unread_count: usize,
}

impl Conversation {
    pub fn new(contact_jid: String, account_jid: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            contact_jid: contact_jid.clone(),
            account_jid,
            name: None,
            messages: Vec::new(),
            unread_count: 0,
        }
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.contact_jid)
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    pub fn add_message(&mut self, message: Message) {
        if message.direction == super::message::Direction::Incoming {
            self.unread_count += 1;
        }
        self.messages.push(message);
    }

    pub fn mark_read(&mut self) {
        self.unread_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::message::Direction;

    #[test]
    fn conversation_new() {
        let conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        assert_eq!(conversation.contact_jid, "contact@example.com");
        assert_eq!(conversation.account_jid, "me@example.com");
        assert_eq!(conversation.name, None);
        assert!(conversation.messages.is_empty());
        assert_eq!(conversation.unread_count, 0);
        assert!(!conversation.id.is_empty());
    }

    #[test]
    fn conversation_display_name_uses_name() {
        let mut conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        conversation.name = Some("Alice".to_string());
        assert_eq!(conversation.display_name(), "Alice");
    }

    #[test]
    fn conversation_display_name_fallback_to_jid() {
        let conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        assert_eq!(conversation.display_name(), "contact@example.com");
    }

    #[test]
    fn conversation_last_message_and_add_message() {
        let mut conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        assert!(conversation.last_message().is_none());

        let message = Message::new(
            "contact@example.com".to_string(),
            "Hello".to_string(),
            Direction::Incoming,
        );
        conversation.add_message(message.clone());
        assert_eq!(conversation.last_message(), Some(&message));
        assert_eq!(conversation.unread_count, 1);
    }

    #[test]
    fn conversation_add_outgoing_message_does_not_increment_unread() {
        let mut conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        let message = Message::new(
            "me@example.com".to_string(),
            "Hi".to_string(),
            Direction::Outgoing,
        );
        conversation.add_message(message);
        assert_eq!(conversation.unread_count, 0);
        assert_eq!(conversation.messages.len(), 1);
    }

    #[test]
    fn conversation_mark_read() {
        let mut conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        let message = Message::new(
            "contact@example.com".to_string(),
            "Hello".to_string(),
            Direction::Incoming,
        );
        conversation.add_message(message);
        assert_eq!(conversation.unread_count, 1);
        conversation.mark_read();
        assert_eq!(conversation.unread_count, 0);
    }

    #[test]
    fn conversation_serde_roundtrip() {
        let mut conversation =
            Conversation::new("contact@example.com".to_string(), "me@example.com".to_string());
        conversation.name = Some("Alice".to_string());
        conversation.add_message(Message::new(
            "contact@example.com".to_string(),
            "Hi".to_string(),
            Direction::Incoming,
        ));
        let json = serde_json::to_string(&conversation).unwrap();
        let decoded: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(conversation, decoded);
    }
}
