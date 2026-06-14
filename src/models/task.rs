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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_task_default() {
        let task = CalendarTask::default();
        assert_eq!(task.id, "");
        assert_eq!(task.account_id, "");
        assert_eq!(task.calendar_id, "");
        assert_eq!(task.href, "");
        assert_eq!(task.etag, None);
        assert_eq!(task.uid, "");
        assert_eq!(task.title, "");
        assert_eq!(task.due, None);
        assert!(!task.all_day);
        assert_eq!(task.description, "");
        assert_eq!(task.location, "");
        assert_eq!(task.status, "");
        assert_eq!(task.priority, 0);
        assert_eq!(task.percent_complete, 0);
        assert_eq!(task.completed, None);
        assert_eq!(task.raw_ics, "");
    }

    #[test]
    fn calendar_task_serde_roundtrip() {
        let due = Utc::now();
        let completed = Some(due + chrono::Duration::hours(1));
        let task = CalendarTask {
            id: "t1".to_string(),
            account_id: "a1".to_string(),
            calendar_id: "c1".to_string(),
            href: "/cal/t1".to_string(),
            etag: Some("etag1".to_string()),
            uid: "uid1".to_string(),
            title: "Buy milk".to_string(),
            due: Some(due),
            all_day: false,
            description: "2% milk".to_string(),
            location: "Store".to_string(),
            status: "needs-action".to_string(),
            priority: 1,
            percent_complete: 50,
            completed,
            raw_ics: "BEGIN:VTODO".to_string(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let decoded: CalendarTask = serde_json::from_str(&json).unwrap();
        assert_eq!(task, decoded);
    }
}
