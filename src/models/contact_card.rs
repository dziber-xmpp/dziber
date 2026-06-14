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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressbook_default() {
        let addressbook = Addressbook::default();
        assert_eq!(addressbook.id, "");
        assert_eq!(addressbook.account_id, "");
        assert_eq!(addressbook.href, "");
        assert_eq!(addressbook.name, "");
        assert_eq!(addressbook.ctag, None);
    }

    #[test]
    fn contact_card_default() {
        let card = ContactCard::default();
        assert_eq!(card.id, "");
        assert_eq!(card.account_id, "");
        assert_eq!(card.addressbook_id, "");
        assert_eq!(card.href, "");
        assert_eq!(card.etag, None);
        assert_eq!(card.uid, "");
        assert_eq!(card.display_name, "");
        assert_eq!(card.first_name, "");
        assert_eq!(card.last_name, "");
        assert!(card.emails.is_empty());
        assert!(card.phones.is_empty());
        assert_eq!(card.org, "");
        assert_eq!(card.note, "");
        assert_eq!(card.raw_vcard, "");
    }

    #[test]
    fn contact_card_serde_roundtrip() {
        let card = ContactCard {
            id: "cc1".to_string(),
            account_id: "a1".to_string(),
            addressbook_id: "ab1".to_string(),
            href: "/contacts/cc1".to_string(),
            etag: Some("etag1".to_string()),
            uid: "uid1".to_string(),
            display_name: "Alice Smith".to_string(),
            first_name: "Alice".to_string(),
            last_name: "Smith".to_string(),
            emails: vec!["alice@example.com".to_string()],
            phones: vec!["555-1234".to_string()],
            org: "Acme".to_string(),
            note: "Note".to_string(),
            raw_vcard: "BEGIN:VCARD".to_string(),
        };
        let json = serde_json::to_string(&card).unwrap();
        let decoded: ContactCard = serde_json::from_str(&json).unwrap();
        assert_eq!(card, decoded);
    }

    #[test]
    fn addressbook_serde_roundtrip() {
        let addressbook = Addressbook {
            id: "ab1".to_string(),
            account_id: "a1".to_string(),
            href: "/contacts/ab1".to_string(),
            name: "Personal".to_string(),
            ctag: Some("etag1".to_string()),
        };
        let json = serde_json::to_string(&addressbook).unwrap();
        let decoded: Addressbook = serde_json::from_str(&json).unwrap();
        assert_eq!(addressbook, decoded);
    }
}
