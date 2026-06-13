use diesel::prelude::*;

use crate::db::models::{DbEmail, DbFilter, DbMailAccount, DbMailbox};

use crate::db::establish_connection;
use crate::models::account::{
    AuthMode, MailAccount, MailProtocol, ManageSieveConfig, security_from_string, security_to_string,
};
use crate::models::mail::{Email, EmailAddress, Mailbox, MailFilter};

fn serialize_list(list: &[String]) -> String {
    list.join("\x1f")
}

fn deserialize_list(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\x1f').map(|x| x.to_string()).collect()
    }
}

fn serialize_addresses(list: &[EmailAddress]) -> String {
    list.iter()
        .map(|a| {
            let name = a.name.as_deref().unwrap_or("");
            format!("{}\x1e{}", name, a.email)
        })
        .collect::<Vec<_>>()
        .join("\x1f")
}

fn deserialize_addresses(s: &str) -> Vec<EmailAddress> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\x1f')
            .map(|part| {
                let mut split = part.splitn(2, '\x1e');
                let name = split.next().filter(|n| !n.is_empty()).map(|n| n.to_string());
                let email = split.next().unwrap_or("").to_string();
                EmailAddress { name, email }
            })
            .collect()
    }
}

pub fn save_account(account: &MailAccount) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::mail_accounts;

    let mut conn = establish_connection();

    if !account.password.is_empty() {
        crate::secrets::store_password(crate::secrets::SERVICE_MAIL, &account.id, &account.password)?;
    }

    let (auth, admin_user_val, admin_pass_val) = match &account.auth_mode {
        AuthMode::Basic => ("basic", None, None),
        AuthMode::StalwartImpersonation {
            admin_user: adm_user,
            admin_pass: adm_pass,
        } => {
            if !adm_pass.is_empty() {
                crate::secrets::store_password(
                    crate::secrets::SERVICE_MAIL_ADMIN,
                    &account.id,
                    adm_pass,
                )?;
            }
            ("stalwart", Some(adm_user.clone()), Some(String::new()))
        }
    };

    let (
        mail_proto,
        imap_server_val,
        imap_port_val,
        smtp_server_val,
        smtp_port_val,
        security_val,
    ) = match &account.mail_protocol {
        MailProtocol::Jmap => ("jmap", None, None, None, None, None),
        MailProtocol::ImapSmtp {
            imap_server: imap_server_cfg,
            imap_port: imap_port_cfg,
            smtp_server: smtp_server_cfg,
            smtp_port: smtp_port_cfg,
            security: security_cfg,
        } => (
            "imap_smtp",
            Some(imap_server_cfg.clone()),
            Some(*imap_port_cfg as i32),
            Some(smtp_server_cfg.clone()),
            Some(*smtp_port_cfg as i32),
            Some(security_to_string(security_cfg)),
        ),
    };

    let (sieve_server_val, sieve_port_val, sieve_security_val) = account
        .sieve_config
        .as_ref()
        .map(|s| {
            (
                Some(s.server.clone()),
                Some(s.port as i32),
                Some(security_to_string(&s.security)),
            )
        })
        .unwrap_or((None, None, None));

    let db_account = DbMailAccount {
        id: account.id.clone(),
        server_url: account.server_url.clone(),
        username: account.username.clone(),
        password: String::new(),
        auth_mode: auth.to_string(),
        admin_user: admin_user_val,
        admin_pass: admin_pass_val,
        last_sync: None,
        mail_protocol: mail_proto.to_string(),
        imap_server: imap_server_val,
        imap_port: imap_port_val,
        smtp_server: smtp_server_val,
        smtp_port: smtp_port_val,
        security: security_val,
        sieve_server: sieve_server_val,
        sieve_port: sieve_port_val,
        sieve_security: sieve_security_val,
    };

    diesel::replace_into(mail_accounts::table)
        .values(&db_account)
        .execute(&mut conn)?;

    Ok(())
}

pub fn load_accounts() -> Result<Vec<MailAccount>, Box<dyn std::error::Error>> {
    use crate::db::schema::mail_accounts;

    let mut conn = establish_connection();
    let results: Vec<DbMailAccount> = mail_accounts::table.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|a| {
            let password = if a.password.is_empty() {
                crate::secrets::get_password(crate::secrets::SERVICE_MAIL, &a.id)
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
                        crate::secrets::get_password(crate::secrets::SERVICE_MAIL_ADMIN, &a.id)
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

            let loaded_security = security_from_string(a.security.as_deref());
            let loaded_mail_protocol = match a.mail_protocol.as_str() {
                "imap_smtp" => {
                    let imap_server_val = a.imap_server.unwrap_or_default();
                    let smtp_server_val = a.smtp_server.unwrap_or_default();
                    let imap_port_val = a
                        .imap_port
                        .map(|p| p as u16)
                        .unwrap_or_else(|| MailProtocol::default_imap_port(&loaded_security));
                    let smtp_port_val = a
                        .smtp_port
                        .map(|p| p as u16)
                        .unwrap_or_else(|| MailProtocol::default_smtp_port(&loaded_security));
                    MailProtocol::ImapSmtp {
                        imap_server: imap_server_val,
                        imap_port: imap_port_val,
                        smtp_server: smtp_server_val,
                        smtp_port: smtp_port_val,
                        security: loaded_security,
                    }
                }
                _ => MailProtocol::Jmap,
            };

            let sieve_config = a.sieve_server.filter(|s| !s.is_empty()).map(|server| {
                ManageSieveConfig {
                    server,
                    port: a.sieve_port.map(|p| p as u16).unwrap_or(4190),
                    security: security_from_string(a.sieve_security.as_deref()),
                }
            });

            MailAccount {
                id: a.id,
                server_url: a.server_url,
                username: a.username,
                password,
                auth_mode: auth,
                mail_protocol: loaded_mail_protocol,
                sieve_config,
            }
        })
        .collect())
}

pub fn delete_account(account_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::mail_accounts::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(mail_accounts.filter(id.eq(account_id))).execute(&mut conn)?;

    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_MAIL, account_id);
    let _ = crate::secrets::delete_password(crate::secrets::SERVICE_MAIL_ADMIN, account_id);

    Ok(())
}

pub fn save_filters(
    acc_id: &str,
    items: &[MailFilter],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::filters::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(filters.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    let db_items: Vec<DbFilter> = items
        .iter()
        .map(|f| DbFilter {
            id: f.id.clone(),
            account_id: f.account_id.clone(),
            name: f.name.clone(),
            content: f.content.clone(),
            is_active: f.is_active,
        })
        .collect();

    diesel::insert_into(filters)
        .values(&db_items)
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_filters(acc_id: &str) -> Result<Vec<MailFilter>, Box<dyn std::error::Error>> {
    use crate::db::schema::filters::dsl::*;

    let mut conn = establish_connection();
    let results: Vec<DbFilter> = filters
        .filter(account_id.eq(acc_id))
        .load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|f| MailFilter {
            id: f.id,
            account_id: f.account_id,
            name: f.name,
            content: f.content,
            is_active: f.is_active,
        })
        .collect())
}

pub fn save_mailboxes(
    acc_id: &str,
    items: &[Mailbox],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::mailboxes::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(mailboxes.filter(account_id.eq(acc_id))).execute(&mut conn)?;

    let db_items: Vec<DbMailbox> = items
        .iter()
        .map(|m| DbMailbox {
            id: m.id.clone(),
            account_id: m.account_id.clone(),
            name: m.name.clone(),
            role: m.role.clone(),
            sort_order: m.sort_order,
            total_emails: m.total_emails,
            unread_emails: m.unread_emails,
        })
        .collect();

    diesel::insert_into(mailboxes)
        .values(&db_items)
        .execute(&mut conn)?;
    Ok(())
}

pub fn load_mailboxes(
    acc_id: &str,
) -> Result<Vec<Mailbox>, Box<dyn std::error::Error>> {
    use crate::db::schema::mailboxes::dsl::*;

    let mut conn = establish_connection();
    let results: Vec<DbMailbox> = mailboxes
        .filter(account_id.eq(acc_id))
        .order(sort_order.asc())
        .load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|m| Mailbox {
            id: m.id,
            account_id: m.account_id,
            name: m.name,
            role: m.role,
            sort_order: m.sort_order,
            total_emails: m.total_emails,
            unread_emails: m.unread_emails,
        })
        .collect())
}

pub fn save_emails(_account_id: &str, items: &[Email]) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::emails::dsl::*;

    let mut conn = establish_connection();

    for item in items {
        let db_email = DbEmail {
            id: item.id.clone(),
            account_id: item.account_id.clone(),
            thread_id: item.thread_id.clone(),
            mailbox_ids: serialize_list(&item.mailbox_ids),
            from_list: serialize_addresses(&item.from),
            to_list: serialize_addresses(&item.to),
            cc_list: serialize_addresses(&item.cc),
            bcc_list: serialize_addresses(&item.bcc),
            subject: item.subject.clone(),
            received_at: item.received_at.naive_utc(),
            preview: item.preview.clone(),
            body_text: item.body_text.clone(),
            body_html: item.body_html.clone(),
            keywords: serialize_list(&item.keywords),
            has_attachments: item.has_attachments,
            size: item.size as i32,
        };

        diesel::replace_into(emails)
            .values(&db_email)
            .execute(&mut conn)?;
    }

    Ok(())
}

pub fn load_emails(
    acc_id: &str,
    mailbox_id: Option<&str>,
) -> Result<Vec<Email>, Box<dyn std::error::Error>> {
    use crate::db::schema::emails::dsl::*;

    let mut conn = establish_connection();
    let mut query = emails
        .filter(account_id.eq(acc_id))
        .order(received_at.desc())
        .into_boxed();

    if let Some(mb) = mailbox_id {
        query = query.filter(mailbox_ids.like(format!("%{}%", mb)));
    }

    let results: Vec<DbEmail> = query.load(&mut conn)?;

    Ok(results
        .into_iter()
        .map(|e| Email {
            id: e.id,
            account_id: e.account_id,
            thread_id: e.thread_id,
            mailbox_ids: deserialize_list(&e.mailbox_ids),
            from: deserialize_addresses(&e.from_list),
            to: deserialize_addresses(&e.to_list),
            cc: deserialize_addresses(&e.cc_list),
            bcc: deserialize_addresses(&e.bcc_list),
            subject: e.subject,
            received_at: e.received_at.and_utc(),
            preview: e.preview,
            body_text: e.body_text,
            body_html: e.body_html,
            keywords: deserialize_list(&e.keywords),
            has_attachments: e.has_attachments,
            size: e.size as i64,
        })
        .collect())
}

pub fn update_email_keywords(
    email_id: &str,
    keywords_list: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::emails::dsl::*;

    let mut conn = establish_connection();
    diesel::update(emails.filter(id.eq(email_id)))
        .set(keywords.eq(serialize_list(keywords_list)))
        .execute(&mut conn)?;
    Ok(())
}

pub fn delete_email(email_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::db::schema::emails::dsl::*;

    let mut conn = establish_connection();
    diesel::delete(emails.filter(id.eq(email_id))).execute(&mut conn)?;
    Ok(())
}
