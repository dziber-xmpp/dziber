use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mailbox {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub role: Option<String>,
    pub sort_order: i32,
    pub total_emails: i32,
    pub unread_emails: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Email {
    pub id: String,
    pub account_id: String,
    pub thread_id: String,
    pub mailbox_ids: Vec<String>,
    pub from: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub bcc: Vec<EmailAddress>,
    pub subject: String,
    pub received_at: DateTime<Utc>,
    pub preview: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub keywords: Vec<String>,
    pub has_attachments: bool,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MailFilter {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub content: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub email: String,
}

impl Email {
    pub fn is_read(&self) -> bool {
        self.keywords.contains(&"$seen".to_string())
    }

    pub fn set_read(&mut self, read: bool) {
        if read {
            if !self.keywords.contains(&"$seen".to_string()) {
                self.keywords.push("$seen".to_string());
            }
        } else {
            self.keywords.retain(|k| k != "$seen");
        }
    }
}
