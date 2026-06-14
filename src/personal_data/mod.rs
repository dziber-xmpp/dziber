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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::account::{AuthMode, ServerAccount};
    use base64::Engine;

    struct MockAccount {
        id: String,
        server_url: String,
        username: String,
        password: String,
        auth_mode: AuthMode,
    }

    impl ServerAccount for MockAccount {
        fn id(&self) -> &str {
            &self.id
        }
        fn server_url(&self) -> &str {
            &self.server_url
        }
        fn username(&self) -> &str {
            &self.username
        }
        fn password(&self) -> &str {
            &self.password
        }
        fn auth_mode(&self) -> &AuthMode {
            &self.auth_mode
        }
    }

    fn basic_account() -> MockAccount {
        MockAccount {
            id: "acc-1".to_string(),
            server_url: "https://example.com".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            auth_mode: AuthMode::Basic,
        }
    }

    #[test]
    fn basic_auth_header_encodes_credentials() {
        let header = basic_auth_header("alice", "secret");
        assert!(header.starts_with("Basic "));
        let encoded = &header["Basic ".len()..];
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD.decode(encoded).unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, "alice:secret");
    }

    #[test]
    fn auth_for_account_basic() {
        let account = basic_account();
        assert_eq!(
            auth_for_account(&account),
            ("alice".to_string(), "secret".to_string())
        );
    }

    #[test]
    fn auth_for_account_stalwart_impersonation() {
        let account = MockAccount {
            auth_mode: AuthMode::StalwartImpersonation {
                admin_user: "admin".to_string(),
                admin_pass: "adminpass".to_string(),
            },
            ..basic_account()
        };
        assert_eq!(
            auth_for_account(&account),
            ("alice%admin".to_string(), "adminpass".to_string())
        );
    }

    #[test]
    fn auth_header_for_account_basic() {
        let account = basic_account();
        let header = auth_header_for_account(&account);
        assert!(header.starts_with("Basic "));
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(&header["Basic ".len()..])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, "alice:secret");
    }
}
