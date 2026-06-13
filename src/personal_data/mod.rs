pub mod calendar;
pub mod contacts;
pub mod dav;
pub mod imap_smtp;
pub mod jmap;
pub mod mail;
pub mod sieve;

use crate::models::account::{AuthMode, ServerAccount};

#[derive(Debug, Clone)]
pub enum PersonalDataEvent {
    SyncFinished(Result<String, String>),
    Emails(Box<[crate::models::mail::Email]>),
    EmailBody(Box<crate::models::mail::Email>),
    Contacts(Box<[crate::models::contact_card::ContactCard]>),
    Events(Box<[crate::models::event::CalendarEvent]>),
    Filters(Box<[crate::models::mail::MailFilter]>),
}

pub fn basic_auth_header(username: &str, password: &str) -> String {
    use base64::Engine;
    let credentials = format!("{}:{}", username, password);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes()))
}

pub fn auth_for_account(account: &impl ServerAccount) -> (String, String) {
    match account.auth_mode() {
        AuthMode::Basic => (account.username().to_string(), account.password().to_string()),
        AuthMode::StalwartImpersonation { admin_user, admin_pass } => {
            let username = format!("{}%{}", account.username(), admin_user);
            (username, admin_pass.clone())
        }
    }
}

pub fn auth_header_for_account(account: &impl ServerAccount) -> String {
    let (username, password) = auth_for_account(account);
    basic_auth_header(&username, &password)
}
