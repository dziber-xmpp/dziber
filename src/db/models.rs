use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::db::schema::{
    messages, omemo_account, omemo_bundle_cache, omemo_devices, omemo_key, omemo_sessions,
    omemo_trust,
};
use crate::models::message::{Direction, Message, MessageStatus};

#[derive(Queryable, Insertable)]
#[diesel(table_name = messages)]
pub struct DbMessage {
    pub id: String,
    pub account_jid: String,
    pub contact_jid: String,
    pub from_jid: String,
    pub body: String,
    pub timestamp: NaiveDateTime,
    pub status: String,
    pub direction: String,
}

impl DbMessage {
    pub fn from_message(msg: &Message, account_jid: &str, contact_jid: &str) -> Self {
        Self {
            id: msg.id.clone(),
            account_jid: account_jid.to_string(),
            contact_jid: contact_jid.to_string(),
            from_jid: msg.from.clone(),
            body: msg.body.clone(),
            timestamp: msg.timestamp.naive_utc(),
            status: match msg.status {
                MessageStatus::Pending => "pending",
                MessageStatus::Sent => "sent",
                MessageStatus::Delivered => "delivered",
                MessageStatus::Received => "received",
                MessageStatus::Error => "error",
            }
            .to_string(),
            direction: match msg.direction {
                Direction::Incoming => "incoming",
                Direction::Outgoing => "outgoing",
            }
            .to_string(),
        }
    }

    pub fn to_message(&self) -> Message {
        Message {
            id: self.id.clone(),
            from: self.from_jid.clone(),
            body: self.body.clone(),
            timestamp: self.timestamp.and_utc(),
            status: match self.status.as_str() {
                "sent" => MessageStatus::Sent,
                "delivered" => MessageStatus::Delivered,
                "received" => MessageStatus::Received,
                "error" => MessageStatus::Error,
                _ => MessageStatus::Pending,
            },
            direction: match self.direction.as_str() {
                "outgoing" => Direction::Outgoing,
                _ => Direction::Incoming,
            },
        }
    }
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = omemo_account)]
pub struct DbOmemoAccount {
    pub id: i32,
    pub device_id: i32,
    pub pickle: Vec<u8>,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = omemo_key)]
pub struct DbOmemoKey {
    pub id: i32,
    pub key: Vec<u8>,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = omemo_sessions)]
pub struct DbOmemoSession {
    pub jid: String,
    pub device_id: i32,
    pub pickle: Vec<u8>,
    pub created_at: NaiveDateTime,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = omemo_devices)]
pub struct DbOmemoDevice {
    pub jid: String,
    pub device_id: i32,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = omemo_trust)]
pub struct DbOmemoTrust {
    pub jid: String,
    pub device_id: i32,
    pub status: String,
}

#[derive(Queryable, Insertable)]
#[diesel(table_name = omemo_bundle_cache)]
pub struct DbOmemoBundleCache {
    pub jid: String,
    pub device_id: i32,
    pub identity_key: Vec<u8>,
}
