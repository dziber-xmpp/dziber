use diesel::prelude::*;

use crate::db::establish_connection;
use crate::db::models::{DbCalendar, DbCalendarAccount, DbEvent, DbTask};

use crate::models::account::{AuthMode, CalendarAccount, dav_or_jmap_from_string, dav_or_jmap_to_string};
use crate::models::calendar::Calendar;
use crate::models::event::CalendarEvent;
use crate::models::task::CalendarTask;

pub fn save_account(account: &CalendarAccount) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::calendar_accounts;

    let mut conn = establish_connection();

    if !account.password.is_empty() {
        crate::secrets::store_password(
            crate::secrets::SERVICE_CALENDAR,
            &account.id,
            &account.password,
        )?;
    }

    let (auth, admin_user_val, admin_pass_val) = match &account.auth_mode {
        AuthMode::Basic => ("basic", None, None),
        AuthMode::StalwartImpersonation {
            admin_user: adm_user,
            admin_pass: adm_pass,
        } => {
            if !adm_pass.is_empty() {
                crate::secrets::store_password(
                    crate::secrets::SERVICE_CALENDAR_ADMIN,
                    &account.id,
                    adm_pass,
                )?;
            }
            ("stalwart", Some(adm_user.clone()), Some(String::new()))
        }
    };

    let db_account = DbCalendarAccount {
        id: account.id.clone(),
        server_url: account.server_url.clone(),
        username: account.username.clone(),
        password: String::new(),
        auth_mode: auth.to_string(),
        admin_user: admin_user_val,
        admin_pass: admin_pass_val,
        last_sync: None,
        calendar_protocol: dav_or_jmap_to_string(&account.calendar_protocol),
    };

    diesel::replace_into(calendar_accounts::table)
        .values(&db_account)
        .execute(&mut conn)?;

    Ok(())
}

pub fn load_accounts() -> Result<Vec<CalendarAccount>, Box<dyn std::error::Error>> {
    use crate::db::schema::calendar_accounts;

    let mut conn = establish_connection();
    let results: Vec<DbCalendarAccount> = calendar_accounts::table.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|a| {
            let password = if a.password.is_empty() {
                crate::secrets::get_password(crate::secrets::SERVICE_CALENDAR, &a.id)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            } else {
                a.password
            };

            let auth = match a.auth_mode.as_str() {
                "stalwart" => {
                    let admin_user = a.admin_user.unwrap_or_default();
                    let admin_pass = if a.admin_pass.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                        crate::secrets::get_password(crate::secrets::SERVICE_CALENDAR_ADMIN, &a.id)
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                    } else {
                        a.admin_pass.unwrap_or_default()
                    };
                    AuthMode::StalwartImpersonation { admin_user, admin_pass }
                }
                _ => AuthMode::Basic,
            };

            CalendarAccount {
                id: a.id,
                server_url: a.server_url,
                username: a.username,
                password,
                auth_mode: auth,
                calendar_protocol: dav_or_jmap_from_string(&a.calendar_protocol),
            }
        })
        .collect())
}

pub fn delete_account(account_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::calendar_accounts::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(calendar_accounts.filter(id.eq(account_id))).execute(&mut conn)?;

    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_CALENDAR, account_id);
    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_CALENDAR_ADMIN, account_id);

    Ok(())
}

pub fn save_calendars(
    acc_id: &str,
    items: &[Calendar],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::calendars::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(calendars.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    let db_items: Vec<DbCalendar> = items
        .iter()
        .map(|c| DbCalendar {
            id: c.id.clone(),
            account_id: c.account_id.clone(),
            href: c.href.clone(),
            name: c.name.clone(),
            color: c.color.clone(),
            ctag: c.ctag.clone(),
        })
        .collect();

    diesel::insert_into(calendars)
        .values(&db_items)
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_calendars(acc_id: &str) -> Result<Vec<Calendar>, Box<dyn std::error::Error>> {
    use crate::db::schema::calendars::dsl::*;

    let mut conn = establish_connection();
    let results: Vec<DbCalendar> = calendars
        .filter(account_id.eq(acc_id))
        .order(name.asc())
        .load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|c| Calendar {
            id: c.id,
            account_id: c.account_id,
            href: c.href,
            name: c.name,
            color: c.color,
            ctag: c.ctag,
        })
        .collect())
}

pub fn save_events(
    acc_id: &str,
    items: &[CalendarEvent],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::events::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(events.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    for item in items {
        let db_event = DbEvent {
            id: item.id.clone(),
            account_id: item.account_id.clone(),
            calendar_id: item.calendar_id.clone(),
            href: item.href.clone(),
            etag: item.etag.clone(),
            uid: item.uid.clone(),
            title: item.title.clone(),
            start: item.start.naive_utc(),
            end: item.end.naive_utc(),
            all_day: item.all_day,
            description: item.description.clone(),
            location: item.location.clone(),
            status: item.status.clone(),
            raw_ics: item.raw_ics.clone(),
        };

        diesel::replace_into(events)
            .values(&db_event)
            .execute(&mut conn)?;
    }

    Ok(())
}

pub fn load_events(
    acc_id: &str,
    cal_id: Option<&str>,
) -> Result<Vec<CalendarEvent>, Box<dyn std::error::Error>> {
    use crate::db::schema::events::dsl::*;

    let mut conn = establish_connection();
    let mut query = events
        .filter(account_id.eq(acc_id))
        .order(start.asc())
        .into_boxed();

    if let Some(c) = cal_id {
        query = query.filter(calendar_id.eq(c));
    }

    let results: Vec<DbEvent> = query.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|e| CalendarEvent {
            id: e.id,
            account_id: e.account_id,
            calendar_id: e.calendar_id,
            href: e.href,
            etag: e.etag,
            uid: e.uid,
            title: e.title,
            start: e.start.and_utc(),
            end: e.end.and_utc(),
            all_day: e.all_day,
            description: e.description,
            location: e.location,
            status: e.status,
            raw_ics: e.raw_ics,
        })
        .collect())
}


pub fn save_tasks(
    acc_id: &str,
    items: &[CalendarTask],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::tasks::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(tasks.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    for item in items {
        let db_task = DbTask {
            id: item.id.clone(),
            account_id: item.account_id.clone(),
            calendar_id: item.calendar_id.clone(),
            href: item.href.clone(),
            etag: item.etag.clone(),
            uid: item.uid.clone(),
            title: item.title.clone(),
            due: item.due.map(|d| d.naive_utc()),
            all_day: item.all_day,
            description: item.description.clone(),
            location: item.location.clone(),
            status: item.status.clone(),
            priority: item.priority,
            percent_complete: item.percent_complete,
            completed: item.completed.map(|d| d.naive_utc()),
            raw_ics: item.raw_ics.clone(),
        };

        diesel::replace_into(tasks)
            .values(&db_task)
            .execute(&mut conn)?;
    }

    Ok(())
}

pub fn load_tasks(
    acc_id: &str,
    cal_id: Option<&str>,
) -> Result<Vec<CalendarTask>, Box<dyn std::error::Error>> {
    use crate::db::schema::tasks::dsl::*;

    let mut conn = establish_connection();
    let mut query = tasks
        .filter(account_id.eq(acc_id))
        .order(due.asc())
        .into_boxed();

    if let Some(c) = cal_id {
        query = query.filter(calendar_id.eq(c));
    }

    let results: Vec<DbTask> = query.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|t| CalendarTask {
            id: t.id,
            account_id: t.account_id,
            calendar_id: t.calendar_id,
            href: t.href,
            etag: t.etag,
            uid: t.uid,
            title: t.title,
            due: t.due.map(|d| d.and_utc()),
            all_day: t.all_day,
            description: t.description,
            location: t.location,
            status: t.status,
            priority: t.priority,
            percent_complete: t.percent_complete,
            completed: t.completed.map(|d| d.and_utc()),
            raw_ics: t.raw_ics,
        })
        .collect())
}


#[cfg(test)]
mod tests {
    use chrono::Utc;
    use diesel::prelude::*;

    use crate::db::models::{DbCalendar, DbCalendarAccount};
    use crate::db::schema::{calendar_accounts, calendars};
    use crate::db::test_helpers::{connection, with_test_db};
    use crate::models::calendar::Calendar;
    use crate::models::event::CalendarEvent;
    use crate::models::task::CalendarTask;

    fn insert_calendar_account(account_id: &str) {
        let mut conn = connection();
        diesel::insert_into(calendar_accounts::table)
            .values(&DbCalendarAccount {
                id: account_id.to_string(),
                server_url: "https://calendar.example.com".to_string(),
                username: "user".to_string(),
                password: String::new(),
                auth_mode: "basic".to_string(),
                admin_user: None,
                admin_pass: None,
                last_sync: None,
                calendar_protocol: "dav".to_string(),
            })
            .execute(&mut conn)
            .unwrap();
    }

    fn insert_calendar(id: &str, account_id: &str) {
        let mut conn = connection();
        diesel::insert_into(calendars::table)
            .values(&DbCalendar {
                id: id.to_string(),
                account_id: account_id.to_string(),
                href: format!("/calendars/{}/", id),
                name: format!("Calendar {}", id),
                color: Some("#ff0000".to_string()),
                ctag: Some("1".to_string()),
            })
            .execute(&mut conn)
            .unwrap();
    }

    fn sample_calendar(id: &str, account_id: &str, name: &str) -> Calendar {
        Calendar {
            id: id.to_string(),
            account_id: account_id.to_string(),
            href: format!("/calendars/{}/", id),
            name: name.to_string(),
            color: Some("#00ff00".to_string()),
            ctag: Some("2".to_string()),
        }
    }

    fn sample_event(id: &str, account_id: &str, calendar_id: &str, title: &str) -> CalendarEvent {
        let start = Utc::now();
        CalendarEvent {
            id: id.to_string(),
            account_id: account_id.to_string(),
            calendar_id: calendar_id.to_string(),
            href: format!("/events/{}", id),
            etag: Some("etag".to_string()),
            uid: format!("uid-{}", id),
            title: title.to_string(),
            start,
            end: start + chrono::Duration::hours(1),
            all_day: false,
            description: "description".to_string(),
            location: "location".to_string(),
            status: "confirmed".to_string(),
            raw_ics: "ICS".to_string(),
        }
    }

    fn sample_task(id: &str, account_id: &str, calendar_id: &str, title: &str) -> CalendarTask {
        CalendarTask {
            id: id.to_string(),
            account_id: account_id.to_string(),
            calendar_id: calendar_id.to_string(),
            href: format!("/tasks/{}", id),
            etag: Some("etag".to_string()),
            uid: format!("uid-{}", id),
            title: title.to_string(),
            due: Some(Utc::now()),
            all_day: false,
            description: "description".to_string(),
            location: "location".to_string(),
            status: "needs-action".to_string(),
            priority: 1,
            percent_complete: 50,
            completed: None,
            raw_ics: "ICS".to_string(),
        }
    }

    #[test]
    fn save_and_load_calendars() {
        let _guard = with_test_db();
        insert_calendar_account("cal-acc-1");

        let items = vec![
            sample_calendar("cal2", "cal-acc-1", "Work"),
            sample_calendar("cal1", "cal-acc-1", "Personal"),
        ];
        super::save_calendars("cal-acc-1", &items).unwrap();
        let loaded = super::load_calendars("cal-acc-1").unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Personal");
        assert_eq!(loaded[1].name, "Work");
        assert_eq!(loaded, vec![
            sample_calendar("cal1", "cal-acc-1", "Personal"),
            sample_calendar("cal2", "cal-acc-1", "Work"),
        ]);
    }

    #[test]
    fn save_and_load_events() {
        let _guard = with_test_db();
        insert_calendar_account("cal-acc-1");
        insert_calendar("cal-1", "cal-acc-1");

        let events = vec![
            sample_event("e1", "cal-acc-1", "cal-1", "Meeting"),
            sample_event("e2", "cal-acc-1", "cal-1", "Lunch"),
        ];
        super::save_events("cal-acc-1", &events).unwrap();
        let loaded = super::load_events("cal-acc-1", None).unwrap();

        assert_eq!(loaded.len(), 2);
        for (original, loaded_event) in events.iter().zip(loaded.iter()) {
            assert_eq!(original.id, loaded_event.id);
            assert_eq!(original.title, loaded_event.title);
            assert_eq!(original.calendar_id, loaded_event.calendar_id);
        }
    }

    #[test]
    fn load_events_by_calendar() {
        let _guard = with_test_db();
        insert_calendar_account("cal-acc-1");
        insert_calendar("cal-1", "cal-acc-1");
        insert_calendar("cal-2", "cal-acc-1");

        let event_a = sample_event("e1", "cal-acc-1", "cal-1", "A");
        let event_b = sample_event("e2", "cal-acc-1", "cal-2", "B");
        super::save_events("cal-acc-1", &[event_a, event_b]).unwrap();

        let loaded = super::load_events("cal-acc-1", Some("cal-2")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "e2");
    }

    #[test]
    fn save_and_load_tasks() {
        let _guard = with_test_db();
        insert_calendar_account("cal-acc-1");
        insert_calendar("cal-1", "cal-acc-1");

        let tasks = vec![
            sample_task("t1", "cal-acc-1", "cal-1", "Task 1"),
            CalendarTask {
                completed: Some(Utc::now()),
                ..sample_task("t2", "cal-acc-1", "cal-1", "Task 2")
            },
        ];
        super::save_tasks("cal-acc-1", &tasks).unwrap();
        let loaded = super::load_tasks("cal-acc-1", None).unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "t1");
        assert_eq!(loaded[1].id, "t2");
        assert!(loaded[1].completed.is_some());
    }

    #[test]
    fn load_tasks_by_calendar() {
        let _guard = with_test_db();
        insert_calendar_account("cal-acc-1");
        insert_calendar("cal-1", "cal-acc-1");
        insert_calendar("cal-2", "cal-acc-1");

        let task_a = sample_task("t1", "cal-acc-1", "cal-1", "A");
        let task_b = sample_task("t2", "cal-acc-1", "cal-2", "B");
        super::save_tasks("cal-acc-1", &[task_a, task_b]).unwrap();

        let loaded = super::load_tasks("cal-acc-1", Some("cal-1")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "t1");
    }
}
