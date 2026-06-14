use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub jid: String,
    pub name: Option<String>,
    pub subscription: Subscription,
    pub groups: Vec<String>,
    pub presence: Presence,
}

impl Contact {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.jid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Subscription {
    #[default]
    None,
    To,
    From,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub show: Show,
    pub status: Option<String>,
    pub available: bool,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            show: Show::None,
            status: None,
            available: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Show {
    None,
    Away,
    Chat,
    Dnd,
    Xa,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_display_name_uses_name() {
        let contact = Contact {
            jid: "user@example.com".to_string(),
            name: Some("Alice".to_string()),
            subscription: Subscription::Both,
            groups: vec!["Friends".to_string()],
            presence: Presence::default(),
        };
        assert_eq!(contact.display_name(), "Alice");
    }

    #[test]
    fn contact_display_name_fallback_to_jid() {
        let contact = Contact {
            jid: "user@example.com".to_string(),
            name: None,
            subscription: Subscription::None,
            groups: vec![],
            presence: Presence::default(),
        };
        assert_eq!(contact.display_name(), "user@example.com");
    }

    #[test]
    fn subscription_default_is_none() {
        assert_eq!(Subscription::default(), Subscription::None);
    }

    #[test]
    fn presence_default() {
        let presence = Presence::default();
        assert_eq!(presence.show, Show::None);
        assert_eq!(presence.status, None);
        assert!(!presence.available);
    }

    #[test]
    fn contact_serde_roundtrip() {
        let contact = Contact {
            jid: "user@example.com".to_string(),
            name: Some("Alice".to_string()),
            subscription: Subscription::Both,
            groups: vec!["Friends".to_string()],
            presence: Presence {
                show: Show::Chat,
                status: Some("Available".to_string()),
                available: true,
            },
        };
        let json = serde_json::to_string(&contact).unwrap();
        let decoded: Contact = serde_json::from_str(&json).unwrap();
        assert_eq!(contact, decoded);
    }
}
