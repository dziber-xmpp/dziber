use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub jid: String,
    pub password: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub omemo_prefs: HashMap<String, bool>,
    pub status: ConnectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_data: Option<PersonalDataConfig>,
}

impl Account {
    pub fn new(jid: String, password: String) -> Self {
        Self {
            jid,
            password,
            display_name: None,
            omemo_prefs: HashMap::new(),
            status: ConnectionStatus::Offline,
            personal_data: None,
        }
    }
}

pub trait ServerAccount {
    fn id(&self) -> &str;
    fn server_url(&self) -> &str;
    fn username(&self) -> &str;
    fn password(&self) -> &str;
    fn auth_mode(&self) -> &AuthMode;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManageSieveConfig {
    pub server: String,
    #[serde(default = "default_sieve_port")]
    pub port: u16,
    #[serde(default)]
    pub security: MailSecurity,
}

fn default_sieve_port() -> u16 {
    4190
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    pub mail_protocol: MailProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sieve_config: Option<ManageSieveConfig>,
}

impl ServerAccount for MailAccount {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    pub contacts_protocol: DavOrJmap,
}

impl ServerAccount for ContactsAccount {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAccount {
    pub id: String,
    pub server_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    pub calendar_protocol: DavOrJmap,
}

impl ServerAccount for CalendarAccount {
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

/// Legacy combined personal-data config. Kept only for migrating old config.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalDataConfig {
    pub server_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub auth_mode: AuthMode,
    #[serde(default)]
    pub mail_protocol: MailProtocol,
    #[serde(default)]
    pub contacts_protocol: DavOrJmap,
    #[serde(default)]
    pub calendar_protocol: DavOrJmap,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Basic,
    StalwartImpersonation {
        admin_user: String,
        admin_pass: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum MailProtocol {
    #[default]
    Jmap,
    ImapSmtp {
        imap_server: String,
        #[serde(default)]
        imap_port: u16,
        smtp_server: String,
        #[serde(default)]
        smtp_port: u16,
        #[serde(default)]
        security: MailSecurity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailSecurity {
    #[default]
    Tls,
    StartTls,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DavOrJmap {
    #[default]
    Dav,
    Jmap,
}

impl MailProtocol {
    pub fn default_imap_port(security: &MailSecurity) -> u16 {
        match security {
            MailSecurity::Tls => 993,
            MailSecurity::StartTls | MailSecurity::None => 143,
        }
    }

    pub fn default_smtp_port(security: &MailSecurity) -> u16 {
        match security {
            MailSecurity::Tls => 465,
            MailSecurity::StartTls => 587,
            MailSecurity::None => 25,
        }
    }
}

pub fn security_to_string(security: &MailSecurity) -> String {
    match security {
        MailSecurity::Tls => "tls".to_string(),
        MailSecurity::StartTls => "starttls".to_string(),
        MailSecurity::None => "none".to_string(),
    }
}

pub fn security_from_string(s: Option<&str>) -> MailSecurity {
    match s {
        Some("starttls") => MailSecurity::StartTls,
        Some("none") => MailSecurity::None,
        _ => MailSecurity::Tls,
    }
}

pub fn dav_or_jmap_to_string(protocol: &DavOrJmap) -> String {
    match protocol {
        DavOrJmap::Dav => "dav".to_string(),
        DavOrJmap::Jmap => "jmap".to_string(),
    }
}

pub fn dav_or_jmap_from_string(s: &str) -> DavOrJmap {
    match s {
        "jmap" => DavOrJmap::Jmap,
        _ => DavOrJmap::Dav,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Offline,
    Connecting,
    Online,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_new() {
        let account = Account::new("user@example.com".to_string(), "secret".to_string());
        assert_eq!(account.jid, "user@example.com");
        assert_eq!(account.password, "secret");
        assert_eq!(account.display_name, None);
        assert!(account.omemo_prefs.is_empty());
        assert_eq!(account.status, ConnectionStatus::Offline);
        assert_eq!(account.personal_data, None);
    }

    #[test]
    fn account_serde_roundtrip() {
        let account = Account::new("user@example.com".to_string(), "secret".to_string());
        let json = serde_json::to_string(&account).unwrap();
        let decoded: Account = serde_json::from_str(&json).unwrap();
        assert_eq!(account, decoded);
    }

    #[test]
    fn mail_account_server_account_trait() {
        let account = MailAccount {
            id: "m1".to_string(),
            server_url: "imap.example.com".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            auth_mode: AuthMode::Basic,
            mail_protocol: MailProtocol::Jmap,
            sieve_config: None,
        };
        assert_eq!(account.id(), "m1");
        assert_eq!(account.server_url(), "imap.example.com");
        assert_eq!(account.username(), "user");
        assert_eq!(account.password(), "pass");
        assert_eq!(account.auth_mode(), &AuthMode::Basic);
    }

    #[test]
    fn contacts_account_server_account_trait() {
        let account = ContactsAccount {
            id: "c1".to_string(),
            server_url: "carddav.example.com".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            auth_mode: AuthMode::Basic,
            contacts_protocol: DavOrJmap::Dav,
        };
        assert_eq!(account.id(), "c1");
        assert_eq!(account.server_url(), "carddav.example.com");
        assert_eq!(account.username(), "user");
        assert_eq!(account.password(), "pass");
        assert_eq!(account.auth_mode(), &AuthMode::Basic);
    }

    #[test]
    fn calendar_account_server_account_trait() {
        let account = CalendarAccount {
            id: "cal1".to_string(),
            server_url: "caldav.example.com".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            auth_mode: AuthMode::Basic,
            calendar_protocol: DavOrJmap::Jmap,
        };
        assert_eq!(account.id(), "cal1");
        assert_eq!(account.server_url(), "caldav.example.com");
        assert_eq!(account.username(), "user");
        assert_eq!(account.password(), "pass");
        assert_eq!(account.auth_mode(), &AuthMode::Basic);
    }

    #[test]
    fn manage_sieve_config_default_port_and_security() {
        let config: ManageSieveConfig =
            serde_json::from_str(r#"{"server":"sieve.example.com"}"#).unwrap();
        assert_eq!(config.server, "sieve.example.com");
        assert_eq!(config.port, 4190);
        assert_eq!(config.security, MailSecurity::Tls);
    }

    #[test]
    fn manage_sieve_config_serde_roundtrip() {
        let config = ManageSieveConfig {
            server: "sieve.example.com".to_string(),
            port: 4190,
            security: MailSecurity::StartTls,
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: ManageSieveConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
    }

    #[test]
    fn personal_data_config_serde_roundtrip() {
        let config = PersonalDataConfig {
            server_url: "example.com".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            auth_mode: AuthMode::StalwartImpersonation {
                admin_user: "admin".to_string(),
                admin_pass: "adminpass".to_string(),
            },
            mail_protocol: MailProtocol::ImapSmtp {
                imap_server: "imap.example.com".to_string(),
                imap_port: 993,
                smtp_server: "smtp.example.com".to_string(),
                smtp_port: 587,
                security: MailSecurity::StartTls,
            },
            contacts_protocol: DavOrJmap::Dav,
            calendar_protocol: DavOrJmap::Jmap,
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: PersonalDataConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
    }

    #[test]
    fn auth_mode_default_is_basic() {
        assert_eq!(AuthMode::default(), AuthMode::Basic);
    }

    #[test]
    fn auth_mode_stalwart_serde_roundtrip() {
        let auth = AuthMode::StalwartImpersonation {
            admin_user: "admin".to_string(),
            admin_pass: "secret".to_string(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let decoded: AuthMode = serde_json::from_str(&json).unwrap();
        assert_eq!(auth, decoded);
    }

    #[test]
    fn mail_protocol_default_is_jmap() {
        assert_eq!(MailProtocol::default(), MailProtocol::Jmap);
    }

    #[test]
    fn mail_security_default_is_tls() {
        assert_eq!(MailSecurity::default(), MailSecurity::Tls);
    }

    #[test]
    fn dav_or_jmap_default_is_dav() {
        assert_eq!(DavOrJmap::default(), DavOrJmap::Dav);
    }

    #[test]
    fn default_imap_port() {
        assert_eq!(MailProtocol::default_imap_port(&MailSecurity::Tls), 993);
        assert_eq!(MailProtocol::default_imap_port(&MailSecurity::StartTls), 143);
        assert_eq!(MailProtocol::default_imap_port(&MailSecurity::None), 143);
    }

    #[test]
    fn default_smtp_port() {
        assert_eq!(MailProtocol::default_smtp_port(&MailSecurity::Tls), 465);
        assert_eq!(MailProtocol::default_smtp_port(&MailSecurity::StartTls), 587);
        assert_eq!(MailProtocol::default_smtp_port(&MailSecurity::None), 25);
    }

    #[test]
    fn security_to_string_works() {
        assert_eq!(security_to_string(&MailSecurity::Tls), "tls");
        assert_eq!(security_to_string(&MailSecurity::StartTls), "starttls");
        assert_eq!(security_to_string(&MailSecurity::None), "none");
    }

    #[test]
    fn security_from_string_works() {
        assert_eq!(security_from_string(Some("tls")), MailSecurity::Tls);
        assert_eq!(security_from_string(Some("starttls")), MailSecurity::StartTls);
        assert_eq!(security_from_string(Some("none")), MailSecurity::None);
        assert_eq!(security_from_string(None), MailSecurity::Tls);
        assert_eq!(security_from_string(Some("unknown")), MailSecurity::Tls);
    }

    #[test]
    fn security_string_roundtrip() {
        for security in [MailSecurity::Tls, MailSecurity::StartTls, MailSecurity::None] {
            assert_eq!(super::security_from_string(Some(&super::security_to_string(&security))), security);
        }
    }

    #[test]
    fn dav_or_jmap_to_string_works() {
        assert_eq!(dav_or_jmap_to_string(&DavOrJmap::Dav), "dav");
        assert_eq!(dav_or_jmap_to_string(&DavOrJmap::Jmap), "jmap");
    }

    #[test]
    fn dav_or_jmap_from_string_works() {
        assert_eq!(dav_or_jmap_from_string("dav"), DavOrJmap::Dav);
        assert_eq!(dav_or_jmap_from_string("jmap"), DavOrJmap::Jmap);
        assert_eq!(dav_or_jmap_from_string("unknown"), DavOrJmap::Dav);
    }

    #[test]
    fn dav_or_jmap_string_roundtrip() {
        for protocol in [DavOrJmap::Dav, DavOrJmap::Jmap] {
            assert_eq!(super::dav_or_jmap_from_string(&super::dav_or_jmap_to_string(&protocol)), protocol);
        }
    }

    #[test]
    fn mail_protocol_imap_smtp_serde_roundtrip() {
        let protocol = MailProtocol::ImapSmtp {
            imap_server: "imap.example.com".to_string(),
            imap_port: 993,
            smtp_server: "smtp.example.com".to_string(),
            smtp_port: 587,
            security: MailSecurity::StartTls,
        };
        let json = serde_json::to_string(&protocol).unwrap();
        let decoded: MailProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(protocol, decoded);
    }

    #[test]
    fn connection_status_serde_roundtrip() {
        let statuses = [
            ConnectionStatus::Offline,
            ConnectionStatus::Connecting,
            ConnectionStatus::Online,
            ConnectionStatus::Error("boom".to_string()),
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: ConnectionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, decoded);
        }
    }
}
