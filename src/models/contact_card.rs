use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Addressbook {
    pub id: String,
    pub account_id: String,
    pub href: String,
    pub name: String,
    pub ctag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ContactCard {
    pub id: String,
    pub account_id: String,
    pub addressbook_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: String,
    pub note: String,
    pub raw_vcard: String,
}
