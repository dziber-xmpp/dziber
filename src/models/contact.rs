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
