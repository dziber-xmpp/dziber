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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_default() {
        let calendar = Calendar::default();
        assert_eq!(calendar.id, "");
        assert_eq!(calendar.account_id, "");
        assert_eq!(calendar.href, "");
        assert_eq!(calendar.name, "");
        assert_eq!(calendar.color, None);
        assert_eq!(calendar.ctag, None);
    }

    #[test]
    fn calendar_serde_roundtrip() {
        let calendar = Calendar {
            id: "c1".to_string(),
            account_id: "a1".to_string(),
            href: "/cal/c1".to_string(),
            name: "Work".to_string(),
            color: Some("#ff0000".to_string()),
            ctag: Some("tag1".to_string()),
        };
        let json = serde_json::to_string(&calendar).unwrap();
        let decoded: Calendar = serde_json::from_str(&json).unwrap();
        assert_eq!(calendar, decoded);
    }
}
