use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalendarEvent {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub description: String,
    pub location: String,
    pub status: String,
    pub raw_ics: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_event_default() {
        let event = CalendarEvent::default();
        assert_eq!(event.id, "");
        assert_eq!(event.account_id, "");
        assert_eq!(event.calendar_id, "");
        assert_eq!(event.href, "");
        assert_eq!(event.etag, None);
        assert_eq!(event.uid, "");
        assert_eq!(event.title, "");
        assert!(!event.all_day);
        assert_eq!(event.description, "");
        assert_eq!(event.location, "");
        assert_eq!(event.status, "");
        assert_eq!(event.raw_ics, "");
    }

    #[test]
    fn calendar_event_serde_roundtrip() {
        let start = Utc::now();
        let end = start + chrono::Duration::hours(1);
        let event = CalendarEvent {
            id: "e1".to_string(),
            account_id: "a1".to_string(),
            calendar_id: "c1".to_string(),
            href: "/cal/e1".to_string(),
            etag: Some("etag1".to_string()),
            uid: "uid1".to_string(),
            title: "Meeting".to_string(),
            start,
            end,
            all_day: false,
            description: "Team meeting".to_string(),
            location: "Room 1".to_string(),
            status: "confirmed".to_string(),
            raw_ics: "BEGIN:VEVENT".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: CalendarEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }
}
