use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::db::schema::{
    addressbooks, calendar_accounts, calendars, contacts, contacts_accounts, emails, events,
    filters, mail_accounts, mailboxes, messages, omemo_account, omemo_bundle_cache, omemo_devices,
    omemo_key, omemo_sessions, omemo_trust, tasks,
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

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = mail_accounts)]
pub struct DbMailAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub auth_mode: String,
    pub admin_user: Option<String>,
    pub admin_pass: Option<String>,
    pub last_sync: Option<NaiveDateTime>,
    pub mail_protocol: String,
    pub imap_server: Option<String>,
    pub imap_port: Option<i32>,
    pub smtp_server: Option<String>,
    pub smtp_port: Option<i32>,
    pub security: Option<String>,
    pub sieve_server: Option<String>,
    pub sieve_port: Option<i32>,
    pub sieve_security: Option<String>,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = filters)]
pub struct DbFilter {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub content: String,
    pub is_active: bool,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = contacts_accounts)]
pub struct DbContactsAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub auth_mode: String,
    pub admin_user: Option<String>,
    pub admin_pass: Option<String>,
    pub last_sync: Option<NaiveDateTime>,
    pub contacts_protocol: String,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = calendar_accounts)]
pub struct DbCalendarAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub auth_mode: String,
    pub admin_user: Option<String>,
    pub admin_pass: Option<String>,
    pub last_sync: Option<NaiveDateTime>,
    pub calendar_protocol: String,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = mailboxes)]
pub struct DbMailbox {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub role: Option<String>,
    pub sort_order: i32,
    pub total_emails: i32,
    pub unread_emails: i32,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = emails)]
pub struct DbEmail {
    pub id: String,
    pub account_id: String,
    pub thread_id: String,
    pub mailbox_ids: String,
    pub from_list: String,
    pub to_list: String,
    pub cc_list: String,
    pub bcc_list: String,
    pub subject: String,
    pub received_at: NaiveDateTime,
    pub preview: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub keywords: String,
    pub has_attachments: bool,
    pub size: i32,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = addressbooks)]
pub struct DbAddressbook {
    pub id: String,
    pub account_id: String,
    pub href: String,
    pub name: String,
    pub ctag: Option<String>,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = contacts)]
pub struct DbContact {
    pub id: String,
    pub account_id: String,
    pub addressbook_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
    pub emails: String,
    pub phones: String,
    pub org: String,
    pub note: String,
    pub raw_vcard: String,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = calendars)]
pub struct DbCalendar {
    pub id: String,
    pub account_id: String,
    pub href: String,
    pub name: String,
    pub color: Option<String>,
    pub ctag: Option<String>,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = events)]
pub struct DbEvent {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub title: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub description: String,
    pub location: String,
    pub status: String,
    pub raw_ics: String,
}

#[derive(Queryable, Insertable, AsChangeset)]
#[diesel(table_name = tasks)]
pub struct DbTask {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    pub title: String,
    pub due: Option<NaiveDateTime>,
    pub all_day: bool,
    pub description: String,
    pub location: String,
    pub status: String,
    pub priority: i32,
    pub percent_complete: i32,
    pub completed: Option<NaiveDateTime>,
    pub raw_ics: String,
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use crate::models::message::{Direction, Message, MessageStatus};

    use super::DbMessage;

    fn fixed_timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2024-07-21T14:32:10.123456789Z")
            .unwrap()
            .to_utc()
    }

    fn sample_message(status: MessageStatus, direction: Direction) -> Message {
        Message {
            id: "msg-1".to_string(),
            from: "from@example.com".to_string(),
            body: "Hello, world!".to_string(),
            timestamp: fixed_timestamp(),
            status,
            direction,
        }
    }

    fn status_str(status: &MessageStatus) -> &'static str {
        match status {
            MessageStatus::Pending => "pending",
            MessageStatus::Sent => "sent",
            MessageStatus::Delivered => "delivered",
            MessageStatus::Received => "received",
            MessageStatus::Error => "error",
        }
    }

    fn direction_str(direction: &Direction) -> &'static str {
        match direction {
            Direction::Incoming => "incoming",
            Direction::Outgoing => "outgoing",
        }
    }

    #[test]
    fn db_message_roundtrip_all_status_and_direction_variants() {
        let statuses = [
            MessageStatus::Pending,
            MessageStatus::Sent,
            MessageStatus::Delivered,
            MessageStatus::Received,
            MessageStatus::Error,
        ];
        let directions = [Direction::Incoming, Direction::Outgoing];

        for status in &statuses {
            for direction in &directions {
                let original = sample_message(status.clone(), direction.clone());
                let db = DbMessage::from_message(&original, "acc@example.com", "contact@example.com");

                assert_eq!(db.id, original.id);
                assert_eq!(db.account_jid, "acc@example.com");
                assert_eq!(db.contact_jid, "contact@example.com");
                assert_eq!(db.from_jid, original.from);
                assert_eq!(db.body, original.body);
                assert_eq!(db.timestamp, original.timestamp.naive_utc());
                assert_eq!(db.status, status_str(status));
                assert_eq!(db.direction, direction_str(direction));

                let round_trip = db.to_message();
                assert_eq!(original, round_trip);
            }
        }
    }

    #[test]
    fn db_message_unknown_status_defaults_to_pending() {
        let db = DbMessage {
            id: "msg-unknown".to_string(),
            account_jid: "acc@example.com".to_string(),
            contact_jid: "contact@example.com".to_string(),
            from_jid: "from@example.com".to_string(),
            body: "body".to_string(),
            timestamp: fixed_timestamp().naive_utc(),
            status: "bogus".to_string(),
            direction: "outgoing".to_string(),
        };
        let msg = db.to_message();
        assert_eq!(msg.status, MessageStatus::Pending);
        assert_eq!(msg.direction, Direction::Outgoing);
    }

    #[test]
    fn db_message_unknown_direction_defaults_to_incoming() {
        let db = DbMessage {
            id: "msg-unknown".to_string(),
            account_jid: "acc@example.com".to_string(),
            contact_jid: "contact@example.com".to_string(),
            from_jid: "from@example.com".to_string(),
            body: "body".to_string(),
            timestamp: fixed_timestamp().naive_utc(),
            status: "sent".to_string(),
            direction: "sideways".to_string(),
        };
        let msg = db.to_message();
        assert_eq!(msg.status, MessageStatus::Sent);
        assert_eq!(msg.direction, Direction::Incoming);
    }
}
