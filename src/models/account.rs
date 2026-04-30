use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub jid: String,
    pub password: String,
    pub display_name: Option<String>,
    pub status: ConnectionStatus,
}

impl Account {
    pub fn new(jid: String, password: String) -> Self {
        Self {
            jid,
            password,
            display_name: None,
            status: ConnectionStatus::Offline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Offline,
    Connecting,
    Online,
    Error(String),
}
