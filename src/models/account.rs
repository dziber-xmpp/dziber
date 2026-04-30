use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub jid: String,
    pub password: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub omemo_prefs: HashMap<String, bool>,
    pub status: ConnectionStatus,
}

impl Account {
    pub fn new(jid: String, password: String) -> Self {
        Self {
            jid,
            password,
            display_name: None,
            omemo_prefs: HashMap::new(),
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
