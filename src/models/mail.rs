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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_default() {
        let mailbox = Mailbox::default();
        assert_eq!(mailbox.id, "");
        assert_eq!(mailbox.account_id, "");
        assert_eq!(mailbox.name, "");
        assert_eq!(mailbox.role, None);
        assert_eq!(mailbox.sort_order, 0);
        assert_eq!(mailbox.total_emails, 0);
        assert_eq!(mailbox.unread_emails, 0);
    }

    #[test]
    fn email_default() {
        let email = Email::default();
        assert_eq!(email.id, "");
        assert!(email.keywords.is_empty());
        assert!(!email.is_read());
    }

    #[test]
    fn email_address_default() {
        let address = EmailAddress::default();
        assert_eq!(address.name, None);
        assert_eq!(address.email, "");
    }

    #[test]
    fn email_is_read_and_set_read() {
        let mut email = Email::default();
        assert!(!email.is_read());

        email.set_read(true);
        assert!(email.is_read());

        email.set_read(true);
        assert_eq!(email.keywords.iter().filter(|k| *k == "$seen").count(), 1);

        email.set_read(false);
        assert!(!email.is_read());

        email.set_read(false);
        assert!(!email.is_read());
    }

    #[test]
    fn mail_filter_default() {
        let filter = MailFilter::default();
        assert_eq!(filter.id, "");
        assert_eq!(filter.account_id, "");
        assert_eq!(filter.name, "");
        assert_eq!(filter.content, "");
        assert!(!filter.is_active);
    }

    #[test]
    fn email_serde_roundtrip() {
        let email = Email {
            id: "e1".to_string(),
            account_id: "a1".to_string(),
            thread_id: "t1".to_string(),
            mailbox_ids: vec!["m1".to_string()],
            from: vec![EmailAddress {
                name: Some("Alice".to_string()),
                email: "alice@example.com".to_string(),
            }],
            to: vec![EmailAddress {
                name: None,
                email: "bob@example.com".to_string(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Hello".to_string(),
            received_at: Utc::now(),
            preview: "Hi".to_string(),
            body_text: Some("Hello Bob".to_string()),
            body_html: None,
            keywords: vec!["$seen".to_string()],
            has_attachments: false,
            size: 42,
        };
        let json = serde_json::to_string(&email).unwrap();
        let decoded: Email = serde_json::from_str(&json).unwrap();
        assert_eq!(email, decoded);
    }

    #[test]
    fn mailbox_serde_roundtrip() {
        let mailbox = Mailbox {
            id: "m1".to_string(),
            account_id: "a1".to_string(),
            name: "Inbox".to_string(),
            role: Some("inbox".to_string()),
            sort_order: 1,
            total_emails: 10,
            unread_emails: 2,
        };
        let json = serde_json::to_string(&mailbox).unwrap();
        let decoded: Mailbox = serde_json::from_str(&json).unwrap();
        assert_eq!(mailbox, decoded);
    }

    #[test]
    fn mail_filter_serde_roundtrip() {
        let filter = MailFilter {
            id: "f1".to_string(),
            account_id: "a1".to_string(),
            name: "Important".to_string(),
            content: "require [\"fileinto\"];".to_string(),
            is_active: true,
        };
        let json = serde_json::to_string(&filter).unwrap();
        let decoded: MailFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(filter, decoded);
    }
}
