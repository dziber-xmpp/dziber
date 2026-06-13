use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Calendar {
    pub id: String,
    pub account_id: String,
    pub href: String,
    pub name: String,
    pub color: Option<String>,
    pub ctag: Option<String>,
}
