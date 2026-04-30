use std::path::PathBuf;

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

use crate::db::models::DbMessage;
use crate::db::schema::messages;
use crate::models::message::Message;

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
