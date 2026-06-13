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
