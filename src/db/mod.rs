use std::cell::RefCell;
use std::path::PathBuf;

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

thread_local! {
    static THREAD_LOCAL_DB_PATH: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set a per-thread SQLite database path used by `establish_connection()`.
///
/// This is intended for tests and example programs that need isolated DBs
/// per worker thread. Pass `None` to clear the override.
pub fn set_thread_local_db_path(path: Option<String>) {
    THREAD_LOCAL_DB_PATH.with(|p| *p.borrow_mut() = path);
}

/// Run `f` with `path` as the thread-local DB override, restoring the previous
/// value afterwards.
pub fn with_thread_local_db_path<R>(path: &str, f: impl FnOnce() -> R) -> R {
    let previous = THREAD_LOCAL_DB_PATH.with(|p| p.borrow().clone());
    set_thread_local_db_path(Some(path.to_string()));
    let result = f();
    set_thread_local_db_path(previous);
    result
}

use crate::db::models::DbMessage;
use crate::db::schema::messages;
use crate::models::message::{Message, MessageStatus};

pub mod calendar;
pub mod contacts;
pub mod mail;
pub mod models;
pub mod omemo;
pub mod schema;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

fn db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dziber")
        .join("dziber.db")
}

pub fn establish_connection() -> SqliteConnection {
    if let Some(path) = THREAD_LOCAL_DB_PATH.with(|p| p.borrow().clone()) {
        return SqliteConnection::establish(&path)
            .expect("Failed to connect to thread-local test SQLite database");
    }
    if let Ok(path) = std::env::var("DZIBER_TEST_DB_PATH") {
        return SqliteConnection::establish(&path)
            .expect("Failed to connect to test SQLite database");
    }

    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    SqliteConnection::establish(path.to_str().expect("Invalid db path"))
        .expect("Failed to connect to SQLite database")
}

pub fn run_migrations() -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| format!("Migration failed: {}", e))?;
    Ok(())
}

pub fn save_message(
    msg: &Message,
    account_jid: &str,
    contact_jid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let db_msg = DbMessage::from_message(msg, account_jid, contact_jid);
    diesel::insert_into(messages::table)
        .values(&db_msg)
        .on_conflict(messages::id)
        .do_nothing()
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_messages(acc_jid: &str) -> Result<Vec<(String, Message)>, Box<dyn std::error::Error>> {
    use crate::db::schema::messages::dsl::*;

    let mut conn = establish_connection();
    let results: Vec<DbMessage> = messages
        .filter(account_jid.eq(acc_jid))
        .order(timestamp.asc())
        .load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|db| (db.contact_jid.clone(), db.to_message()))
        .collect())
}

pub fn purge_history() -> Result<usize, Box<dyn std::error::Error>> {
    let mut conn = establish_connection();
    let deleted = diesel::delete(messages::table).execute(&mut conn)?;
    Ok(deleted)
}

pub fn update_message_status(
    message_id: &str,
    status_value: MessageStatus,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::messages::dsl::*;

    let mut conn = establish_connection();
    let status_str = match status_value {
        MessageStatus::Pending => "pending",
        MessageStatus::Sent => "sent",
        MessageStatus::Delivered => "delivered",
        MessageStatus::Received => "received",
        MessageStatus::Error => "error",
    };
    diesel::update(messages.filter(id.eq(message_id)))
        .set(status.eq(status_str))
        .execute(&mut conn)?;
    Ok(())
}

pub fn update_message_body(
    message_id: &str,
    body_value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::messages::dsl::*;

    let mut conn = establish_connection();
    diesel::update(messages.filter(id.eq(message_id)))
        .set(body.eq(body_value))
        .execute(&mut conn)?;
    Ok(())
}

#[cfg(test)]
pub mod test_helpers {
    use std::sync::{Mutex, MutexGuard};

    use diesel::sqlite::SqliteConnection;
    use tempfile::TempDir;

    use super::{establish_connection, run_migrations};

    static DB_TEST_MUTEX: Mutex<()> = Mutex::new(());

    pub struct TestDbGuard {
        _lock: MutexGuard<'static, ()>,
        _temp_dir: TempDir,
        old_path: Option<String>,
    }

    impl Drop for TestDbGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(old) = self.old_path.take() {
                    std::env::set_var("DZIBER_TEST_DB_PATH", old);
                } else {
                    let _ = std::env::remove_var("DZIBER_TEST_DB_PATH");
                }
            }
        }
    }

    pub fn with_test_db() -> TestDbGuard {
        let lock = DB_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = temp_dir.path().join("dziber.db");
        let old_path = std::env::var("DZIBER_TEST_DB_PATH").ok();
        unsafe {
            std::env::set_var(
                "DZIBER_TEST_DB_PATH",
                db_path.to_str().expect("invalid temp db path"),
            );
        }
        run_migrations().expect("failed to run migrations");
        TestDbGuard {
            _lock: lock,
            _temp_dir: temp_dir,
            old_path,
        }
    }

    pub fn connection() -> SqliteConnection {
        establish_connection()
    }
}

#[cfg(test)]
mod tests {
    use crate::db::test_helpers::with_test_db;
    use crate::models::message::{Direction, Message, MessageStatus};

    fn sample_message(id: &str, status: MessageStatus, direction: Direction, body: &str) -> Message {
        Message {
            id: id.to_string(),
            from: "from@example.com".to_string(),
            body: body.to_string(),
            timestamp: chrono::DateTime::parse_from_rfc3339("2024-08-01T10:00:00Z")
                .unwrap()
                .to_utc(),
            status,
            direction,
        }
    }

    #[test]
    fn run_migrations_succeeds() {
        let _guard = with_test_db();
    }

    #[test]
    fn save_and_load_messages_roundtrip() {
        let _guard = with_test_db();

        let msg1 = sample_message("m1", MessageStatus::Pending, Direction::Incoming, "Hi");
        let msg2 = sample_message("m2", MessageStatus::Sent, Direction::Outgoing, "Bye");
        super::save_message(&msg1, "acc@example.com", "contact@example.com").unwrap();
        super::save_message(&msg2, "acc@example.com", "contact@example.com").unwrap();

        let loaded = super::load_messages("acc@example.com").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].0, "contact@example.com");
        assert_eq!(loaded[0].1, msg1);
        assert_eq!(loaded[1].1, msg2);
    }

    #[test]
    fn save_message_is_idempotent() {
        let _guard = with_test_db();

        let msg = sample_message("m1", MessageStatus::Pending, Direction::Incoming, "Hi");
        super::save_message(&msg, "acc@example.com", "contact@example.com").unwrap();
        super::save_message(&msg, "acc@example.com", "contact@example.com").unwrap();

        let loaded = super::load_messages("acc@example.com").unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn update_message_status_roundtrip() {
        let _guard = with_test_db();

        let msg = sample_message("m1", MessageStatus::Pending, Direction::Incoming, "Hi");
        super::save_message(&msg, "acc@example.com", "contact@example.com").unwrap();
        super::update_message_status("m1", MessageStatus::Delivered).unwrap();

        let loaded = super::load_messages("acc@example.com").unwrap();
        assert_eq!(loaded[0].1.status, MessageStatus::Delivered);
    }

    #[test]
    fn update_message_body_roundtrip() {
        let _guard = with_test_db();

        let msg = sample_message("m1", MessageStatus::Pending, Direction::Incoming, "Hi");
        super::save_message(&msg, "acc@example.com", "contact@example.com").unwrap();
        super::update_message_body("m1", "Updated body").unwrap();

        let loaded = super::load_messages("acc@example.com").unwrap();
        assert_eq!(loaded[0].1.body, "Updated body");
    }

    #[test]
    fn purge_history_deletes_all_messages() {
        let _guard = with_test_db();

        let msg1 = sample_message("m1", MessageStatus::Pending, Direction::Incoming, "Hi");
        let msg2 = sample_message("m2", MessageStatus::Sent, Direction::Outgoing, "Bye");
        super::save_message(&msg1, "acc@example.com", "contact@example.com").unwrap();
        super::save_message(&msg2, "acc@example.com", "other@example.com").unwrap();

        let deleted = super::purge_history().unwrap();
        assert_eq!(deleted, 2);

        let loaded = super::load_messages("acc@example.com").unwrap();
        assert!(loaded.is_empty());
    }
}
