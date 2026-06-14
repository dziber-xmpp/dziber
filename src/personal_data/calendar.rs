use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{Value, json};

use crate::models::account::CalendarAccount;
use crate::models::calendar::Calendar;
use crate::models::event::CalendarEvent;
use crate::models::task::CalendarTask;
use crate::personal_data::auth_header_for_account;
use crate::personal_data::dav::{DavClient, encode_account, extract_rel_path};
use crate::personal_data::jmap::JmapClient;

pub struct CalDavClient {
    dav: DavClient,
    account: CalendarAccount,
}

impl CalDavClient {
    pub fn new(account: &CalendarAccount) -> Self {
        let auth_header = auth_header_for_account(account);
        let dav = DavClient::new(account.server_url.clone(), auth_header);
        Self {
            dav,
            account: account.clone(),
        }
    }

    fn caldav_root(&self) -> String {
        let encoded = encode_account(&self.account.username);
        format!("/dav/cal/{}/", encoded)
    }

    async fn root(&self) -> String {
        self.dav
            .discover_home_set("caldav")
            .await
            .unwrap_or_else(|| self.caldav_root())
    }

    fn account_id(&self) -> String {
        self.account.id.clone()
    }

    pub async fn list_calendars(&self) -> Result<Vec<Calendar>, String> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop><D:resourcetype/><D:displayname/><D:calendar-color/><D:getctag/></D:prop>
</D:propfind>"#;

        let root = self.root().await;
        let responses = self
            .dav
            .propfind(&root, body, "1")
            .await
            .map_err(|e| e.to_string())?;

        let account_id = self.account_id();
        let mut calendars = Vec::new();

        for resp in responses {
            let href = resp.href.clone();
            if href.ends_with('/') && resp.resource_type().iter().any(|t| t == "calendar") {
                let rel = extract_rel_path(&href, "cal");
                let name = resp.prop("displayname").unwrap_or("Calendar").to_string();
                let color = resp.prop("calendar-color").map(|s| s.to_string());
                let ctag = resp.prop("getctag").map(|s| s.to_string());
                calendars.push(Calendar {
                    id: rel.trim_end_matches('/').to_string(),
                    account_id: account_id.clone(),
                    href,
                    name,
                    color,
                    ctag,
                });
            }
        }

        Ok(calendars)
    }

    pub async fn list_events(
        &self,
        calendar: &Calendar,
        year: i32,
    ) -> Result<Vec<CalendarEvent>, String> {
        self.query_calendar(calendar, year, "VEVENT").await
    }

    pub async fn list_tasks(
        &self,
        calendar: &Calendar,
        year: i32,
    ) -> Result<Vec<CalendarTask>, String> {
        self.query_tasks(calendar, year).await
    }

    async fn query_calendar(
        &self,
        calendar: &Calendar,
        year: i32,
        component: &str,
    ) -> Result<Vec<CalendarEvent>, String> {
        let path = if calendar.href.starts_with('/') {
            calendar.href.clone()
        } else {
            format!("/{}", calendar.href)
        };

        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/><C:calendar-data/></D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="{}">
        <C:time-range start="{}0101T000000Z" end="{}0101T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#,
            component,
            year,
            year + 1
        );

        let responses = self
            .dav
            .report(&path, &body, "1")
            .await
            .map_err(|e| e.to_string())?;

        let account_id = self.account_id();
        let mut events = Vec::new();

        for resp in responses {
            if resp.href.ends_with(".ics")
                && let Some(ics_text) = resp.prop("calendar-data")
                    && let Some(event) = parse_event(
                        &account_id,
                        &calendar.id,
                        &resp.href,
                        resp.prop("getetag").map(|s| s.to_string()),
                        ics_text,
                    ) {
                        events.push(event);
                    }
        }

        Ok(events)
    }

    async fn query_tasks(
        &self,
        calendar: &Calendar,
        _year: i32,
    ) -> Result<Vec<CalendarTask>, String> {
        let path = if calendar.href.starts_with('/') {
            calendar.href.clone()
        } else {
            format!("/{}", calendar.href)
        };

        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/><C:calendar-data/></D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VTODO"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let responses = self
            .dav
            .report(&path, body, "1")
            .await
            .map_err(|e| e.to_string())?;

        let account_id = self.account_id();
        let mut tasks = Vec::new();

        for resp in responses {
            if resp.href.ends_with(".ics")
                && let Some(ics_text) = resp.prop("calendar-data")
                    && let Some(task) = parse_task(
                        &account_id,
                        &calendar.id,
                        &resp.href,
                        resp.prop("getetag").map(|s| s.to_string()),
                        ics_text,
                    ) {
                        tasks.push(task);
                    }
        }

        Ok(tasks)
    }

    pub async fn save_event(&self, event: &CalendarEvent) -> Result<(), String> {
        let ics_text = serialize_event(event);
        let path = if event.href.starts_with('/') {
            event.href.clone()
        } else {
            format!("/{}", event.href)
        };

        let etag = event.etag.as_deref();
        let (status, _) = self
            .dav
            .put(
                &path,
                ics_text,
                "text/calendar; charset=utf-8",
                etag,
            )
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to save event: HTTP {}", status));
        }

        Ok(())
    }

    pub async fn delete_event(&self, event: &CalendarEvent) -> Result<(), String> {
        let path = if event.href.starts_with('/') {
            event.href.clone()
        } else {
            format!("/{}", event.href)
        };

        let etag = event.etag.as_deref();
        let status = self
            .dav
            .delete(&path, etag)
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to delete event: HTTP {}", status));
        }

        Ok(())
    }

    pub async fn save_task(&self, task: &CalendarTask) -> Result<(), String> {
        let ics_text = serialize_task(task);
        let path = if task.href.starts_with('/') {
            task.href.clone()
        } else {
            format!("/{}", task.href)
        };

        let etag = task.etag.as_deref();
        let (status, _) = self
            .dav
            .put(
                &path,
                ics_text,
                "text/calendar; charset=utf-8",
                etag,
            )
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to save task: HTTP {}", status));
        }

        Ok(())
    }

    pub async fn delete_task(&self, task: &CalendarTask) -> Result<(), String> {
        let path = if task.href.starts_with('/') {
            task.href.clone()
        } else {
            format!("/{}", task.href)
        };

        let etag = task.etag.as_deref();
        let status = self
            .dav
            .delete(&path, etag)
            .await
            .map_err(|e| e.to_string())?;

        if status >= 400 {
            return Err(format!("Failed to delete task: HTTP {}", status));
        }

        Ok(())
    }

}

fn unfold_ics(text: &str) -> String {
    text.replace("\r\n ", "").replace("\r\n\t", "").replace("\n ", "").replace("\n\t", "")
}

fn parse_ics_properties(text: &str) -> Vec<(String, String)> {
    let unfolded = unfold_ics(text);
    unfolded
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(2, ':');
            let name_part = parts.next()?;
            let value = parts.next().unwrap_or("").to_string();
            let name = name_part.split(';').next().unwrap_or(name_part).to_uppercase();
            Some((name, value))
        })
        .collect()
}

fn decode_ics_text(s: &str) -> String {
    s.replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

fn encode_ics_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    if value.len() == 8 {
        // DATE only
        NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()))
    } else if value.ends_with('Z') {
        NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
            .ok()
            .map(|dt| Utc.from_utc_datetime(&dt))
    } else {
        NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            .ok()
            .map(|dt| Utc.from_utc_datetime(&dt))
    }
}

fn format_datetime(dt: DateTime<Utc>, all_day: bool) -> String {
    if all_day {
        dt.format("%Y%m%d").to_string()
    } else {
        dt.format("%Y%m%dT%H%M%SZ").to_string()
    }
}

fn ics_components(text: &str, name: &str) -> Vec<String> {
    let mut components = Vec::new();
    let mut current = String::new();
    let mut in_component = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(&format!("BEGIN:{}", name)) {
            in_component = true;
            current = format!("BEGIN:{}\r\n", name);
        } else if trimmed.eq_ignore_ascii_case(&format!("END:{}", name)) {
            if in_component {
                current.push_str(&format!("END:{}\r\n", name));
                in_component = false;
                components.push(current);
                current = String::new();
            }
        } else if in_component {
            current.push_str(line);
            current.push_str("\r\n");
        }
    }

    components
}

pub fn import_ics(
    text: &str,
    account_id: &str,
    calendar_id: &str,
) -> (Vec<CalendarEvent>, Vec<CalendarTask>) {
    let events = ics_components(text, "VEVENT")
        .into_iter()
        .filter_map(|body| parse_event(account_id, calendar_id, "", None, &body))
        .collect();
    let tasks = ics_components(text, "VTODO")
        .into_iter()
        .filter_map(|body| parse_task(account_id, calendar_id, "", None, &body))
        .collect();
    (events, tasks)
}

pub fn export_ics(events: &[CalendarEvent], tasks: &[CalendarTask]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//dziber//EN".to_string(),
    ];

    for event in events {
        lines.extend(event_component_lines(event));
    }
    for task in tasks {
        lines.extend(task_component_lines(task));
    }

    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n") + "\r\n"
}

fn parse_event(
    account_id: &str,
    calendar_id: &str,
    href: &str,
    etag: Option<String>,
    text: &str,
) -> Option<CalendarEvent> {
    let props = parse_ics_properties(text);
    let mut event = CalendarEvent {
        id: String::new(),
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        href: href.to_string(),
        etag,
        uid: String::new(),
        title: String::new(),
        start: Utc::now(),
        end: Utc::now(),
        all_day: false,
        description: String::new(),
        location: String::new(),
        status: String::new(),
        raw_ics: text.to_string(),
    };

    for (name, value) in props {
        match name.as_str() {
            "UID" => {
                event.uid = value;
            }
            "SUMMARY" => event.title = decode_ics_text(&value),
            "DTSTART" => {
                event.all_day = value.len() == 8;
                if let Some(dt) = parse_datetime(&value) {
                    event.start = dt;
                }
            }
            "DTEND" => {
                if let Some(dt) = parse_datetime(&value) {
                    event.end = dt;
                }
            }
            "DESCRIPTION" => event.description = decode_ics_text(&value),
            "LOCATION" => event.location = decode_ics_text(&value),
            "STATUS" => event.status = value,
            _ => {}
        }
    }

    if event.uid.is_empty() {
        event.uid = href.split('/').next_back().unwrap_or(href).to_string();
    }
    event.id = event.uid.clone();

    Some(event)
}

fn event_component_lines(event: &CalendarEvent) -> Vec<String> {
    vec![
        "BEGIN:VEVENT".to_string(),
        format!("UID:{}", event.uid),
        format!("SUMMARY:{}", encode_ics_text(&event.title)),
        format!(
            "DTSTART{}:{}",
            if event.all_day { ";VALUE=DATE" } else { "" },
            format_datetime(event.start, event.all_day)
        ),
        format!(
            "DTEND{}:{}",
            if event.all_day { ";VALUE=DATE" } else { "" },
            format_datetime(event.end, event.all_day)
        ),
        format!("DESCRIPTION:{}", encode_ics_text(&event.description)),
        format!("LOCATION:{}", encode_ics_text(&event.location)),
        format!(
            "STATUS:{}",
            if event.status.is_empty() { "CONFIRMED" } else { &event.status }
        ),
        "END:VEVENT".to_string(),
    ]
}

fn serialize_event(event: &CalendarEvent) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//dziber//EN".to_string(),
    ];
    lines.extend(event_component_lines(event));
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n") + "\r\n"
}

fn parse_task(
    account_id: &str,
    calendar_id: &str,
    href: &str,
    etag: Option<String>,
    text: &str,
) -> Option<CalendarTask> {
    let props = parse_ics_properties(text);
    let mut task = CalendarTask {
        id: String::new(),
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        href: href.to_string(),
        etag,
        uid: String::new(),
        title: String::new(),
        due: None,
        all_day: false,
        description: String::new(),
        location: String::new(),
        status: String::new(),
        priority: 0,
        percent_complete: 0,
        completed: None,
        raw_ics: text.to_string(),
    };

    for (name, value) in props {
        match name.as_str() {
            "UID" => task.uid = value,
            "SUMMARY" => task.title = decode_ics_text(&value),
            "DUE" => {
                task.all_day = value.len() == 8;
                task.due = parse_datetime(&value);
            }
            "DESCRIPTION" => task.description = decode_ics_text(&value),
            "LOCATION" => task.location = decode_ics_text(&value),
            "STATUS" => task.status = value,
            "PRIORITY" => task.priority = value.parse().unwrap_or(0),
            "PERCENT-COMPLETE" => task.percent_complete = value.parse().unwrap_or(0),
            "COMPLETED" => task.completed = parse_datetime(&value),
            _ => {}
        }
    }

    if task.uid.is_empty() {
        task.uid = href.split('/').next_back().unwrap_or(href).to_string();
    }
    task.id = task.uid.clone();

    Some(task)
}

fn task_component_lines(task: &CalendarTask) -> Vec<String> {
    let mut lines = vec![
        "BEGIN:VTODO".to_string(),
        format!("UID:{}", task.uid),
        format!("SUMMARY:{}", encode_ics_text(&task.title)),
    ];

    if let Some(due) = task.due {
        lines.push(format!(
            "DUE{}:{}",
            if task.all_day { ";VALUE=DATE" } else { "" },
            format_datetime(due, task.all_day)
        ));
    }

    lines.push(format!("DESCRIPTION:{}", encode_ics_text(&task.description)));
    lines.push(format!("LOCATION:{}", encode_ics_text(&task.location)));
    lines.push(format!(
        "STATUS:{}",
        if task.status.is_empty() { "NEEDS-ACTION" } else { &task.status }
    ));
    lines.push(format!("PRIORITY:{}", task.priority));
    lines.push(format!("PERCENT-COMPLETE:{}", task.percent_complete));

    if let Some(completed) = task.completed {
        lines.push(format!("COMPLETED:{}", format_datetime(completed, false)));
    }

    lines.push("END:VTODO".to_string());
    lines
}

fn serialize_task(task: &CalendarTask) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//dziber//EN".to_string(),
    ];
    lines.extend(task_component_lines(task));
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n") + "\r\n"
}

pub struct JmapCalendarClient {
    jmap: JmapClient,
}

impl JmapCalendarClient {
    pub fn new(account: &CalendarAccount) -> Self {
        Self {
            jmap: JmapClient::new(account),
        }
    }

    pub async fn list_calendars(&self) -> Result<Vec<Calendar>, String> {
        let account_id = self.jmap.account_id.clone();
        let response = self
            .jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![json!([
                    "Calendar/get",
                    {
                        "accountId": account_id,
                        "ids": null
                    },
                    "0"
                ])],
            )
            .await?;

        let mut calendars = Vec::new();
        if let Some(args) = self.jmap.extract_response(&response, "Calendar/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        calendars.push(Calendar {
                            id: id.to_string(),
                            account_id: self.jmap.account_id.clone(),
                            href: String::new(),
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Calendar")
                                .to_string(),
                            color: item
                                .get("color")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            ctag: None,
                        });
                    }
                }
            }
        Ok(calendars)
    }

    pub async fn list_events(
        &self,
        calendar: &Calendar,
        year: i32,
    ) -> Result<Vec<CalendarEvent>, String> {
        let account_id = self.jmap.account_id.clone();
        let _start = format!("{}-01-01T00:00:00Z", year);
        let _end = format!("{}-01-01T00:00:00Z", year + 1);

        let response = self
            .jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![json!([
                    "CalendarEvent/get",
                    {
                        "accountId": account_id,
                        "ids": null,
                        "properties": [
                            "id", "calendarIds", "uid", "title", "start", "timeZone",
                            "duration", "end", "showWithoutTime", "description",
                            "location", "status"
                        ]
                    },
                    "0"
                ])],
            )
            .await?;

        let mut events = Vec::new();
        if let Some(args) = self.jmap.extract_response(&response, "CalendarEvent/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(event) = parse_jmap_event(&self.jmap.account_id, item)
                        && event.calendar_id == calendar.id
                            && event.start.year() >= year
                            && event.start.year() <= year + 1
                        {
                            events.push(event);
                        }
                }
            }
        Ok(events)
    }

    pub async fn list_tasks(
        &self,
        calendar: &Calendar,
        _year: i32,
    ) -> Result<Vec<CalendarTask>, String> {
        let account_id = self.jmap.account_id.clone();
        let response = self
            .jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![json!([
                    "Task/get",
                    {
                        "accountId": account_id,
                        "ids": null,
                        "properties": [
                            "id", "calendarIds", "uid", "title", "due", "showWithoutTime",
                            "description", "location", "status", "priority", "percentComplete"
                        ]
                    },
                    "0"
                ])],
            )
            .await?;

        let mut tasks = Vec::new();
        if let Some(args) = self.jmap.extract_response(&response, "Task/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(task) = parse_jmap_task(&self.jmap.account_id, item)
                        && task.calendar_id == calendar.id {
                            tasks.push(task);
                        }
                }
            }
        Ok(tasks)
    }

    pub async fn save_event(&self, event: &CalendarEvent) -> Result<(), String> {
        let account_id = self.jmap.account_id.clone();
        let payload = event_to_jmap_json(event);
        let method_call = if event.id.is_empty() || event.id.starts_with('~') {
            json!([
                "CalendarEvent/set",
                {
                    "accountId": account_id,
                    "create": {
                        "new-event": payload
                    }
                },
                "0"
            ])
        } else {
            json!([
                "CalendarEvent/set",
                {
                    "accountId": account_id,
                    "update": {
                        event.id.clone(): payload
                    }
                },
                "0"
            ])
        };

        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![method_call],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_event(&self, event: &CalendarEvent) -> Result<(), String> {
        if event.id.is_empty() {
            return Ok(());
        }
        let account_id = self.jmap.account_id.clone();
        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![json!([
                    "CalendarEvent/set",
                    {
                        "accountId": account_id,
                        "destroy": [event.id.clone()]
                    },
                    "0"
                ])],
            )
            .await?;
        Ok(())
    }

    pub async fn save_task(&self, task: &CalendarTask) -> Result<(), String> {
        let account_id = self.jmap.account_id.clone();
        let payload = task_to_jmap_json(task);
        let method_call = if task.id.is_empty() || task.id.starts_with('~') {
            json!([
                "Task/set",
                {
                    "accountId": account_id,
                    "create": {
                        "new-task": payload
                    }
                },
                "0"
            ])
        } else {
            json!([
                "Task/set",
                {
                    "accountId": account_id,
                    "update": {
                        task.id.clone(): payload
                    }
                },
                "0"
            ])
        };

        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![method_call],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_task(&self, task: &CalendarTask) -> Result<(), String> {
        if task.id.is_empty() {
            return Ok(());
        }
        let account_id = self.jmap.account_id.clone();
        self.jmap
            .request(
                &[
                    "urn:ietf:params:jmap:core",
                    "urn:ietf:params:jmap:calendars",
                ],
                vec![json!([
                    "Task/set",
                    {
                        "accountId": account_id,
                        "destroy": [task.id.clone()]
                    },
                    "0"
                ])],
            )
            .await?;
        Ok(())
    }

}

fn parse_jmap_datetime(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

fn parse_iso_duration(s: &str) -> Option<Duration> {
    // Minimal ISO 8601 duration parser for PT#H#M#S and P#D.
    let mut secs: i64 = 0;
    if s.starts_with("P") && !s.contains('T') {
        let num: String = s[1..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Some('D') = s.chars().last() {
            let days: i64 = num.parse().ok()?;
            return Some(Duration::days(days));
        }
    }
    if let Some(rest) = s.strip_prefix("PT") {
        let mut value = String::new();
        for ch in rest.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                value.push(ch);
            } else {
                let v: f64 = value.parse().ok()?;
                match ch {
                    'H' => secs += (v * 3600.0) as i64,
                    'M' => secs += (v * 60.0) as i64,
                    'S' => secs += v as i64,
                    _ => {}
                }
                value.clear();
            }
        }
        return Some(Duration::seconds(secs));
    }
    None
}

fn parse_jmap_event(account_id: &str, item: &Value) -> Option<CalendarEvent> {
    let id = item.get("id")?.as_str()?.to_string();
    let calendar_id = item
        .get("calendarIds")
        .and_then(|v| v.as_object())?
        .keys()
        .next()?
        .clone();
    let uid = item
        .get("uid")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    let title = item
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = item
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let location = item
        .get("location")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let all_day = item
        .get("showWithoutTime")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let start_str = item.get("start")?.as_str()?;
    let start = parse_jmap_datetime(start_str)?;

    let end = if let Some(end_str) = item.get("end").and_then(|v| v.as_str()) {
        parse_jmap_datetime(end_str).unwrap_or(start)
    } else if let Some(dur_str) = item.get("duration").and_then(|v| v.as_str()) {
        start + parse_iso_duration(dur_str).unwrap_or(Duration::hours(1))
    } else {
        if all_day {
            start + Duration::days(1)
        } else {
            start + Duration::hours(1)
        }
    };

    Some(CalendarEvent {
        id,
        account_id: account_id.to_string(),
        calendar_id,
        href: String::new(),
        etag: None,
        uid,
        title,
        start,
        end,
        all_day,
        description,
        location,
        status,
        raw_ics: String::new(),
    })
}

fn parse_jmap_task(account_id: &str, item: &Value) -> Option<CalendarTask> {
    let id = item.get("id")?.as_str()?.to_string();
    let calendar_id = item
        .get("calendarIds")
        .and_then(|v| v.as_object())?
        .keys()
        .next()?
        .clone();
    let uid = item
        .get("uid")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    let title = item
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = item
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let location = item
        .get("location")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let all_day = item
        .get("showWithoutTime")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let due = item
        .get("due")
        .and_then(|v| v.as_str())
        .and_then(parse_jmap_datetime);

    let priority = item
        .get("priority")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let percent_complete = item
        .get("percentComplete")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    Some(CalendarTask {
        id,
        account_id: account_id.to_string(),
        calendar_id,
        href: String::new(),
        etag: None,
        uid,
        title,
        due,
        all_day,
        description,
        location,
        status,
        priority,
        percent_complete,
        completed: None,
        raw_ics: String::new(),
    })
}

fn event_to_jmap_json(event: &CalendarEvent) -> Value {
    if event.all_day {
        json!({
            "calendarIds": { event.calendar_id.clone(): true },
            "uid": event.uid.clone(),
            "title": event.title.clone(),
            "start": event.start.format("%Y-%m-%d").to_string(),
            "duration": "P1D",
            "showWithoutTime": true,
            "description": event.description.clone(),
            "location": event.location.clone(),
            "status": event.status.clone()
        })
    } else {
        let duration = event.end - event.start;
        json!({
            "calendarIds": { event.calendar_id.clone(): true },
            "uid": event.uid.clone(),
            "title": event.title.clone(),
            "start": event.start.to_rfc3339(),
            "timeZone": "UTC",
            "duration": duration_to_iso(duration),
            "description": event.description.clone(),
            "location": event.location.clone(),
            "status": event.status.clone()
        })
    }
}

fn task_to_jmap_json(task: &CalendarTask) -> Value {
    let mut obj = json!({
        "calendarIds": { task.calendar_id.clone(): true },
        "uid": task.uid.clone(),
        "title": task.title.clone(),
        "description": task.description.clone(),
        "location": task.location.clone(),
        "status": task.status.clone(),
        "priority": task.priority,
        "percentComplete": task.percent_complete
    });

    if let Some(due) = task.due {
        if task.all_day {
            obj["due"] = json!(due.format("%Y-%m-%d").to_string());
            obj["showWithoutTime"] = json!(true);
        } else {
            obj["due"] = json!(due.to_rfc3339());
        }
    }

    obj
}

fn duration_to_iso(d: Duration) -> String {
    let secs = d.num_seconds();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if mins == 0 && s == 0 {
        format!("PT{}H", hours)
    } else if s == 0 {
        format!("PT{}H{}M", hours, mins)
    } else {
        format!("PT{}H{}M{}S", hours, mins, s)
    }
}

pub enum CalendarClient {
    Dav(CalDavClient),
    Jmap(JmapCalendarClient),
}

impl CalendarClient {
    pub fn new(account: &CalendarAccount) -> Self {
        match account.calendar_protocol {
            crate::models::account::DavOrJmap::Jmap => Self::Jmap(JmapCalendarClient::new(account)),
            _ => Self::Dav(CalDavClient::new(account)),
        }
    }

    pub async fn list_calendars(&self) -> Result<Vec<Calendar>, String> {
        match self {
            Self::Dav(c) => c.list_calendars().await,
            Self::Jmap(c) => c.list_calendars().await,
        }
    }

    pub async fn list_events(
        &self,
        calendar: &Calendar,
        year: i32,
    ) -> Result<Vec<CalendarEvent>, String> {
        match self {
            Self::Dav(c) => c.list_events(calendar, year).await,
            Self::Jmap(c) => c.list_events(calendar, year).await,
        }
    }

    pub async fn list_tasks(
        &self,
        calendar: &Calendar,
        year: i32,
    ) -> Result<Vec<CalendarTask>, String> {
        match self {
            Self::Dav(c) => c.list_tasks(calendar, year).await,
            Self::Jmap(c) => c.list_tasks(calendar, year).await,
        }
    }

    pub async fn save_event(&self, event: &CalendarEvent) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.save_event(event).await,
            Self::Jmap(c) => c.save_event(event).await,
        }
    }

    pub async fn delete_event(&self, event: &CalendarEvent) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.delete_event(event).await,
            Self::Jmap(c) => c.delete_event(event).await,
        }
    }

    pub async fn save_task(&self, task: &CalendarTask) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.save_task(task).await,
            Self::Jmap(c) => c.save_task(task).await,
        }
    }

    pub async fn delete_task(&self, task: &CalendarTask) -> Result<(), String> {
        match self {
            Self::Dav(c) => c.delete_task(task).await,
            Self::Jmap(c) => c.delete_task(task).await,
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn sample_event() -> CalendarEvent {
        CalendarEvent {
            id: "evt-1".to_string(),
            account_id: "a".to_string(),
            calendar_id: "cal-1".to_string(),
            href: String::new(),
            etag: None,
            uid: "evt-1".to_string(),
            title: "Meeting".to_string(),
            start: Utc.with_ymd_and_hms(2026, 6, 15, 10, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 6, 15, 11, 0, 0).unwrap(),
            all_day: false,
            description: "Discuss plans".to_string(),
            location: "Office".to_string(),
            status: "CONFIRMED".to_string(),
            raw_ics: String::new(),
        }
    }

    fn sample_task() -> CalendarTask {
        CalendarTask {
            id: "tsk-1".to_string(),
            account_id: "a".to_string(),
            calendar_id: "cal-1".to_string(),
            href: String::new(),
            etag: None,
            uid: "tsk-1".to_string(),
            title: "Buy milk".to_string(),
            due: Some(Utc.with_ymd_and_hms(2026, 6, 16, 12, 0, 0).unwrap()),
            all_day: false,
            description: "Get milk".to_string(),
            location: String::new(),
            status: "NEEDS-ACTION".to_string(),
            priority: 1,
            percent_complete: 0,
            completed: None,
            raw_ics: String::new(),
        }
    }

    #[test]
    fn ics_export_import_roundtrip() {
        let event = sample_event();
        let task = sample_task();
        let ics = export_ics(std::slice::from_ref(&event), std::slice::from_ref(&task));

        assert!(ics.contains("BEGIN:VCALENDAR"));
        assert!(ics.contains("BEGIN:VEVENT"));
        assert!(ics.contains("BEGIN:VTODO"));

        let (events, tasks) = import_ics(&ics, "a", "cal-1");
        assert_eq!(events.len(), 1);
        assert_eq!(tasks.len(), 1);

        assert_eq!(events[0].uid, event.uid);
        assert_eq!(events[0].title, event.title);
        assert_eq!(events[0].description, event.description);
        assert_eq!(events[0].location, event.location);

        assert_eq!(tasks[0].uid, task.uid);
        assert_eq!(tasks[0].title, task.title);
        assert_eq!(tasks[0].description, task.description);
        assert_eq!(tasks[0].priority, task.priority);
    }

    #[test]
    fn ics_import_multiple_components() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e1\r\nSUMMARY:One\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:e2\r\nSUMMARY:Two\r\nDTSTART:20260616T100000Z\r\nDTEND:20260616T110000Z\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:t1\r\nSUMMARY:Task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let (events, tasks) = import_ics(ics, "a", "cal-1");
        assert_eq!(events.len(), 2);
        assert_eq!(tasks.len(), 1);
        assert_eq!(events[0].title, "One");
        assert_eq!(events[1].title, "Two");
        assert_eq!(tasks[0].title, "Task");
    }

    #[test]
    fn unfold_ics_removes_continuation_whitespace() {
        let folded = "SUMMARY:hello\r\n world\r\nDESCRIPTION:line\n continued";
        assert_eq!(unfold_ics(folded), "SUMMARY:helloworld\r\nDESCRIPTION:linecontinued");
    }

    #[test]
    fn parse_ics_properties_extracts_name_value_pairs() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nUID:evt\r\nSUMMARY:Hello\\, world\r\nDTSTART;VALUE=DATE:20260614\r\nEND:VCALENDAR\r\n";
        let props = parse_ics_properties(ics);
        assert!(props.iter().any(|(k, v)| k == "UID" && v == "evt"));
        assert!(props.iter().any(|(k, v)| k == "SUMMARY" && v == "Hello\\, world"));
        assert!(props.iter().any(|(k, v)| k == "DTSTART" && v == "20260614"));
    }

    #[test]
    fn decode_and_encode_ics_text_are_inverses() {
        let original = "Line one\nLine two, with; commas and \\ backslash";
        let encoded = encode_ics_text(original);
        assert!(encoded.contains("\\n"));
        assert!(encoded.contains("\\,"));
        assert!(encoded.contains("\\;"));
        assert!(encoded.contains("\\\\"));
        assert_eq!(decode_ics_text(&encoded), original);
    }

    #[test]
    fn parse_datetime_handles_date_and_datetime() {
        let date = parse_datetime("20260614").unwrap();
        assert_eq!(date.format("%Y%m%d").to_string(), "20260614");
        assert_eq!(date.hour(), 0);

        let utc = parse_datetime("20260614T123456Z").unwrap();
        assert_eq!(utc.to_rfc3339(), "2026-06-14T12:34:56+00:00");

        let naive = parse_datetime("20260614T123456").unwrap();
        assert_eq!(naive.to_rfc3339(), "2026-06-14T12:34:56+00:00");

        assert!(parse_datetime("not-a-date").is_none());
    }

    #[test]
    fn format_datetime_produces_expected_formats() {
        let dt = Utc.with_ymd_and_hms(2026, 6, 14, 12, 34, 56).unwrap();
        assert_eq!(format_datetime(dt, true), "20260614");
        assert_eq!(format_datetime(dt, false), "20260614T123456Z");
    }

    #[test]
    fn ics_components_extracts_named_blocks() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e1\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:e2\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:t1\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let events = ics_components(ics, "VEVENT");
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("UID:e1"));
        assert!(events[1].contains("UID:e2"));

        let tasks = ics_components(ics, "VTODO");
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].contains("UID:t1"));
    }

    #[test]
    fn parse_iso_duration_converts_common_durations() {
        assert_eq!(parse_iso_duration("P1D"), Some(Duration::days(1)));
        assert_eq!(parse_iso_duration("PT2H30M"), Some(Duration::seconds(9000)));
        assert_eq!(parse_iso_duration("PT45S"), Some(Duration::seconds(45)));
        assert_eq!(parse_iso_duration("PT1.5S"), Some(Duration::seconds(1)));
        assert!(parse_iso_duration("invalid").is_none());
    }

    #[test]
    fn duration_to_iso_formats_seconds() {
        assert_eq!(duration_to_iso(Duration::hours(2)), "PT2H");
        assert_eq!(duration_to_iso(Duration::minutes(90)), "PT1H30M");
        assert_eq!(duration_to_iso(Duration::seconds(3661)), "PT1H1M1S");
    }

    #[test]
    fn parse_jmap_datetime_accepts_multiple_formats() {
        assert_eq!(
            parse_jmap_datetime("2026-06-14T12:34:56Z").map(|d| d.to_rfc3339()),
            Some("2026-06-14T12:34:56+00:00".to_string())
        );
        assert_eq!(
            parse_jmap_datetime("2026-06-14T12:34:56").map(|d| d.to_rfc3339()),
            Some("2026-06-14T12:34:56+00:00".to_string())
        );
        assert_eq!(
            parse_jmap_datetime("2026-06-14").map(|d| d.format("%Y-%m-%d").to_string()),
            Some("2026-06-14".to_string())
        );
        assert!(parse_jmap_datetime("bad").is_none());
    }

    #[test]
    fn event_to_jmap_json_has_required_fields() {
        let event = sample_event();
        let value = event_to_jmap_json(&event);
        assert_eq!(value["uid"], json!(event.uid));
        assert_eq!(value["title"], json!(event.title));
        assert_eq!(value["timeZone"], json!("UTC"));
        assert_eq!(value["duration"], json!("PT1H"));
    }

    #[test]
    fn all_day_event_to_jmap_json_uses_date_and_p1d() {
        let mut event = sample_event();
        event.all_day = true;
        let value = event_to_jmap_json(&event);
        assert_eq!(value["start"], json!(event.start.format("%Y-%m-%d").to_string()));
        assert_eq!(value["duration"], json!("P1D"));
        assert_eq!(value["showWithoutTime"], json!(true));
    }

    #[test]
    fn task_to_jmap_json_has_required_fields() {
        let task = sample_task();
        let value = task_to_jmap_json(&task);
        assert_eq!(value["uid"], json!(task.uid));
        assert_eq!(value["title"], json!(task.title));
        assert_eq!(value["priority"], json!(task.priority));
        assert_eq!(value["percentComplete"], json!(task.percent_complete));
        assert!(value["due"].is_string());
    }
}
