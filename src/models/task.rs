use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalendarTask {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub title: String,
    pub due: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub description: String,
    pub location: String,
    pub status: String,
    pub priority: i32,
    pub percent_complete: i32,
    pub completed: Option<DateTime<Utc>>,
    pub raw_ics: String,
}
