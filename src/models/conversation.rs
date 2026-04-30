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
