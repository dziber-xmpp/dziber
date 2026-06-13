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
