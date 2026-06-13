use iced::widget::{Button, Column, checkbox, pick_list, row, text, text_input};
use iced::Element;

use crate::models::account::{
    AuthMode, CalendarAccount, ContactsAccount, DavOrJmap, MailAccount, MailProtocol, MailSecurity,
    ManageSieveConfig,
};
use crate::ui::app::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountCategory {
    Mail,
    Contacts,
    Calendar,
}

#[derive(Debug, Clone)]
pub enum AccountField {
    ServerUrl(String),
    Username(String),
    Password(String),
    UseImpersonation(bool),
    AdminUser(String),
    AdminPass(String),
    Protocol(String),
    ImapServer(String),
    ImapPort(String),
    SmtpServer(String),
    SmtpPort(String),
    Security(String),
    SieveServer(String),
    SievePort(String),
    SieveSecurity(String),
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Changed(AccountCategory, AccountField),
    SaveClicked,
    ClearClicked,
}

impl SettingsMessage {
    pub fn into_message(self) -> Message {
        Message::Settings(self)
    }
}

#[derive(Debug, Default)]
pub struct AccountFormState {
    pub id: Option<String>,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub use_impersonation: bool,
    pub admin_user: String,
    pub admin_pass: String,
}

impl AccountFormState {
    fn from_account(id: &str, server_url: &str, username: &str, password: &str, auth_mode: &AuthMode) -> Self {
        let mut state = Self {
            id: Some(id.to_string()),
            server_url: server_url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            ..Self::default()
        };
        if let AuthMode::StalwartImpersonation { admin_user, admin_pass } = auth_mode {
            state.use_impersonation = true;
            state.admin_user = admin_user.clone();
            state.admin_pass = admin_pass.clone();
        }
        state
    }

    fn auth_mode(&self) -> AuthMode {
        if self.use_impersonation {
            AuthMode::StalwartImpersonation {
                admin_user: self.admin_user.clone(),
                admin_pass: self.admin_pass.clone(),
            }
        } else {
            AuthMode::Basic
        }
    }

    fn is_filled(&self) -> bool {
        !self.server_url.is_empty() && !self.username.is_empty() && !self.password.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct MailSettingsState {
    pub base: AccountFormState,
    pub protocol: String,
    pub imap_server: String,
    pub imap_port: String,
    pub smtp_server: String,
    pub smtp_port: String,
    pub security: String,
    pub sieve_server: String,
    pub sieve_port: String,
    pub sieve_security: String,
}

impl MailSettingsState {
    pub fn from_account(account: &MailAccount) -> Self {
        let mut state = Self {
            base: AccountFormState::from_account(
                &account.id,
                &account.server_url,
                &account.username,
                &account.password,
                &account.auth_mode,
            ),
            ..Self::default()
        };
        match &account.mail_protocol {
            MailProtocol::Jmap => state.protocol = "jmap".to_string(),
            MailProtocol::ImapSmtp {
                imap_server,
                imap_port,
                smtp_server,
                smtp_port,
                security,
            } => {
                state.protocol = "imap_smtp".to_string();
                state.imap_server = imap_server.clone();
                state.imap_port = imap_port.to_string();
                state.smtp_server = smtp_server.clone();
                state.smtp_port = smtp_port.to_string();
                state.security = match security {
                    MailSecurity::Tls => "tls".to_string(),
                    MailSecurity::StartTls => "starttls".to_string(),
                    MailSecurity::None => "none".to_string(),
                };
            }
        }
        if let Some(sieve) = account.sieve_config.as_ref() {
            state.sieve_server = sieve.server.clone();
            state.sieve_port = sieve.port.to_string();
            state.sieve_security = match sieve.security {
                MailSecurity::Tls => "tls".to_string(),
                MailSecurity::StartTls => "starttls".to_string(),
                MailSecurity::None => "none".to_string(),
            };
        }
        state
    }

    pub fn to_account(&self) -> Option<MailAccount> {
        if !self.base.is_filled() {
            return None;
        }

        let security = match self.security.as_str() {
            "starttls" => MailSecurity::StartTls,
            "none" => MailSecurity::None,
            _ => MailSecurity::Tls,
        };

        let mail_protocol = if self.protocol == "imap_smtp" {
            let imap_port = self
                .imap_port
                .parse::<u16>()
                .unwrap_or_else(|_| MailProtocol::default_imap_port(&security));
            let smtp_port = self
                .smtp_port
                .parse::<u16>()
                .unwrap_or_else(|_| MailProtocol::default_smtp_port(&security));
            MailProtocol::ImapSmtp {
                imap_server: self.imap_server.clone(),
                imap_port,
                smtp_server: self.smtp_server.clone(),
                smtp_port,
                security,
            }
        } else {
            MailProtocol::Jmap
        };

        let sieve_config = if self.sieve_server.is_empty() {
            None
        } else {
            let sieve_security = match self.sieve_security.as_str() {
                "starttls" => MailSecurity::StartTls,
                "none" => MailSecurity::None,
                _ => MailSecurity::Tls,
            };
            Some(ManageSieveConfig {
                server: self.sieve_server.clone(),
                port: self.sieve_port.parse::<u16>().unwrap_or(4190),
                security: sieve_security,
            })
        };

        Some(MailAccount {
            id: self.base.id.clone().unwrap_or_else(new_id),
            server_url: self.base.server_url.clone(),
            username: self.base.username.clone(),
            password: self.base.password.clone(),
            auth_mode: self.base.auth_mode(),
            mail_protocol,
            sieve_config,
        })
    }
}

#[derive(Debug, Default)]
pub struct ProtocolSettingsState {
    pub base: AccountFormState,
    pub protocol: String,
}

impl ProtocolSettingsState {
    pub fn from_contacts(account: &ContactsAccount) -> Self {
        let mut state = Self {
            base: AccountFormState::from_account(
                &account.id,
                &account.server_url,
                &account.username,
                &account.password,
                &account.auth_mode,
            ),
            protocol: match account.contacts_protocol {
                DavOrJmap::Dav => "dav".to_string(),
                DavOrJmap::Jmap => "jmap".to_string(),
            },
        };
        state.base.id = Some(account.id.clone());
        state
    }

    pub fn from_calendar(account: &CalendarAccount) -> Self {
        let mut state = Self {
            base: AccountFormState::from_account(
                &account.id,
                &account.server_url,
                &account.username,
                &account.password,
                &account.auth_mode,
            ),
            protocol: match account.calendar_protocol {
                DavOrJmap::Dav => "dav".to_string(),
                DavOrJmap::Jmap => "jmap".to_string(),
            },
        };
        state.base.id = Some(account.id.clone());
        state
    }

    pub fn to_contacts(&self) -> Option<ContactsAccount> {
        if !self.base.is_filled() {
            return None;
        }
        Some(ContactsAccount {
            id: self.base.id.clone().unwrap_or_else(new_id),
            server_url: self.base.server_url.clone(),
            username: self.base.username.clone(),
            password: self.base.password.clone(),
            auth_mode: self.base.auth_mode(),
            contacts_protocol: protocol_from_string(&self.protocol),
        })
    }

    pub fn to_calendar(&self) -> Option<CalendarAccount> {
        if !self.base.is_filled() {
            return None;
        }
        Some(CalendarAccount {
            id: self.base.id.clone().unwrap_or_else(new_id),
            server_url: self.base.server_url.clone(),
            username: self.base.username.clone(),
            password: self.base.password.clone(),
            auth_mode: self.base.auth_mode(),
            calendar_protocol: protocol_from_string(&self.protocol),
        })
    }
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub mail: MailSettingsState,
    pub contacts: ProtocolSettingsState,
    pub calendar: ProtocolSettingsState,
}

impl SettingsState {
    pub fn from_configs(
        mail: Option<&MailAccount>,
        contacts: Option<&ContactsAccount>,
        calendar: Option<&CalendarAccount>,
    ) -> Self {
        Self {
            mail: mail.map(MailSettingsState::from_account).unwrap_or_default(),
            contacts: contacts
                .map(ProtocolSettingsState::from_contacts)
                .unwrap_or_default(),
            calendar: calendar
                .map(ProtocolSettingsState::from_calendar)
                .unwrap_or_default(),
        }
    }

    pub fn to_configs(
        &self,
    ) -> (
        Option<MailAccount>,
        Option<ContactsAccount>,
        Option<CalendarAccount>,
    ) {
        (
            self.mail.to_account(),
            self.contacts.to_contacts(),
            self.calendar.to_calendar(),
        )
    }

    pub fn update(&mut self, message: SettingsMessage) {
        if let SettingsMessage::Changed(category, field) = message {
            match category {
                AccountCategory::Mail => update_mail(&mut self.mail, field),
                AccountCategory::Contacts => update_protocol(&mut self.contacts, field),
                AccountCategory::Calendar => update_protocol(&mut self.calendar, field),
            }
        }
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn protocol_from_string(s: &str) -> DavOrJmap {
    match s {
        "jmap" => DavOrJmap::Jmap,
        _ => DavOrJmap::Dav,
    }
}

fn update_base(base: &mut AccountFormState, field: AccountField) {
    match field {
        AccountField::ServerUrl(v) => base.server_url = v,
        AccountField::Username(v) => base.username = v,
        AccountField::Password(v) => base.password = v,
        AccountField::UseImpersonation(v) => base.use_impersonation = v,
        AccountField::AdminUser(v) => base.admin_user = v,
        AccountField::AdminPass(v) => base.admin_pass = v,
        _ => {}
    }
}

fn update_mail(state: &mut MailSettingsState, field: AccountField) {
    match field {
        AccountField::Protocol(v) => state.protocol = v,
        AccountField::ImapServer(v) => state.imap_server = v,
        AccountField::ImapPort(v) => state.imap_port = v,
        AccountField::SmtpServer(v) => state.smtp_server = v,
        AccountField::SmtpPort(v) => state.smtp_port = v,
        AccountField::Security(v) => state.security = v,
        AccountField::SieveServer(v) => state.sieve_server = v,
        AccountField::SievePort(v) => state.sieve_port = v,
        AccountField::SieveSecurity(v) => state.sieve_security = v,
        other => update_base(&mut state.base, other),
    }
}

fn update_protocol(state: &mut ProtocolSettingsState, field: AccountField) {
    match field {
        AccountField::Protocol(v) => state.protocol = v,
        other => update_base(&mut state.base, other),
    }
}

fn base_inputs<'a>(
    category: AccountCategory,
    state: &'a AccountFormState,
) -> Vec<Element<'a, Message>> {
    vec![
        text_input(
            "Server URL (https://mail.example.com)",
            &state.server_url,
        )
        .on_input(move |v| {
            SettingsMessage::Changed(category, AccountField::ServerUrl(v)).into_message()
        })
        .into(),
        text_input("Username / Email", &state.username)
            .on_input(move |v| {
                SettingsMessage::Changed(category, AccountField::Username(v)).into_message()
            })
            .into(),
        text_input("Password / App Token", &state.password)
            .secure(true)
            .on_input(move |v| {
                SettingsMessage::Changed(category, AccountField::Password(v)).into_message()
            })
            .into(),
        checkbox(state.use_impersonation)
            .label("Stalwart master-user impersonation")
            .on_toggle(move |v| {
                SettingsMessage::Changed(category, AccountField::UseImpersonation(v))
                    .into_message()
            })
            .into(),
    ]
}

fn admin_inputs<'a>(
    category: AccountCategory,
    state: &'a AccountFormState,
) -> Vec<Element<'a, Message>> {
    vec![
        text_input("Admin user (e.g. %admin)", &state.admin_user)
            .on_input(move |v| {
                SettingsMessage::Changed(category, AccountField::AdminUser(v)).into_message()
            })
            .into(),
        text_input("Admin password", &state.admin_pass)
            .secure(true)
            .on_input(move |v| {
                SettingsMessage::Changed(category, AccountField::AdminPass(v)).into_message()
            })
            .into(),
    ]
}

pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let security_options = vec!["tls".to_string(), "starttls".to_string(), "none".to_string()];
    let dav_jmap_options = vec!["dav".to_string(), "jmap".to_string()];

    let mut content = Column::new()
        .padding(16)
        .spacing(12)
        .push(text("Personal Data Accounts").size(18));

    content = content.push(text("Mail").size(14));
    for el in base_inputs(AccountCategory::Mail, &state.mail.base) {
        content = content.push(el);
    }

    let mail_protocols = vec!["jmap".to_string(), "imap_smtp".to_string()];
    content = content
        .push(text("Mail protocol").size(12))
        .push(pick_list(
            mail_protocols,
            Some(state.mail.protocol.clone()),
            |v| SettingsMessage::Changed(AccountCategory::Mail, AccountField::Protocol(v)).into_message(),
        ));

    if state.mail.protocol == "imap_smtp" {
        content = content
            .push(
                text_input("IMAP server", &state.mail.imap_server).on_input(|v| {
                    SettingsMessage::Changed(AccountCategory::Mail, AccountField::ImapServer(v))
                        .into_message()
                }),
            )
            .push(
                text_input("IMAP port", &state.mail.imap_port).on_input(|v| {
                    SettingsMessage::Changed(AccountCategory::Mail, AccountField::ImapPort(v))
                        .into_message()
                }),
            )
            .push(
                text_input("SMTP server", &state.mail.smtp_server).on_input(|v| {
                    SettingsMessage::Changed(AccountCategory::Mail, AccountField::SmtpServer(v))
                        .into_message()
                }),
            )
            .push(
                text_input("SMTP port", &state.mail.smtp_port).on_input(|v| {
                    SettingsMessage::Changed(AccountCategory::Mail, AccountField::SmtpPort(v))
                        .into_message()
                }),
            )
            .push(text("Connection security").size(12))
            .push(pick_list(
                security_options.clone(),
                Some(state.mail.security.clone()),
                |v| SettingsMessage::Changed(AccountCategory::Mail, AccountField::Security(v)).into_message(),
            ));
    }

    if state.mail.base.use_impersonation {
        for el in admin_inputs(AccountCategory::Mail, &state.mail.base) {
            content = content.push(el);
        }
    }

    content = content
        .push(text("ManageSieve (filters)").size(12))
        .push(
            text_input("Sieve server", &state.mail.sieve_server).on_input(|v| {
                SettingsMessage::Changed(AccountCategory::Mail, AccountField::SieveServer(v))
                    .into_message()
            }),
        )
        .push(
            text_input("Sieve port", &state.mail.sieve_port).on_input(|v| {
                SettingsMessage::Changed(AccountCategory::Mail, AccountField::SievePort(v))
                    .into_message()
            }),
        )
        .push(pick_list(
            security_options,
            Some(state.mail.sieve_security.clone()),
            |v| {
                SettingsMessage::Changed(AccountCategory::Mail, AccountField::SieveSecurity(v))
                    .into_message()
            },
        ));

    content = content.push(text("Contacts").size(14));
    for el in base_inputs(AccountCategory::Contacts, &state.contacts.base) {
        content = content.push(el);
    }
    content = content
        .push(text("Contacts protocol").size(12))
        .push(pick_list(
            dav_jmap_options.clone(),
            Some(state.contacts.protocol.clone()),
            |v| {
                SettingsMessage::Changed(AccountCategory::Contacts, AccountField::Protocol(v))
                    .into_message()
            },
        ));
    if state.contacts.base.use_impersonation {
        for el in admin_inputs(AccountCategory::Contacts, &state.contacts.base) {
            content = content.push(el);
        }
    }

    content = content.push(text("Calendar").size(14));
    for el in base_inputs(AccountCategory::Calendar, &state.calendar.base) {
        content = content.push(el);
    }
    content = content
        .push(text("Calendar protocol").size(12))
        .push(pick_list(
            dav_jmap_options,
            Some(state.calendar.protocol.clone()),
            |v| {
                SettingsMessage::Changed(AccountCategory::Calendar, AccountField::Protocol(v))
                    .into_message()
            },
        ));
    if state.calendar.base.use_impersonation {
        for el in admin_inputs(AccountCategory::Calendar, &state.calendar.base) {
            content = content.push(el);
        }
    }

    content = content.push(
        row![
            Button::new("Save").on_press(SettingsMessage::SaveClicked.into_message()),
            Button::new("Clear").on_press(SettingsMessage::ClearClicked.into_message()),
        ]
        .spacing(8),
    );

    content.into()
}
