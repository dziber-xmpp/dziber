use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_imap::types::Flag;
use chrono::{DateTime, Utc};
use futures::stream::TryStreamExt;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox as SmtpMailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_parser::Address as ParserAddress;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::models::account::{MailAccount, MailProtocol, MailSecurity};
use crate::models::mail::{Email, EmailAddress, Mailbox};

enum ImapConnection {
    Plain(Box<TcpStream>),
    Tls(Box<TlsStream<TcpStream>>),
}

impl fmt::Debug for ImapConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConnection").finish()
    }
}

impl Unpin for ImapConnection {}

impl AsyncRead for ImapConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImapConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub struct ImapSmtpClient {
    account: MailAccount,
}

impl ImapSmtpClient {
    pub fn new(account: &MailAccount) -> Self {
        Self { account: account.clone() }
    }

    fn imap_params(&self) -> Result<(&str, u16, &MailSecurity), String> {
        match &self.account.mail_protocol {
            MailProtocol::ImapSmtp {
                imap_server,
                imap_port,
                security,
                ..
            } => Ok((imap_server.as_str(), *imap_port, security)),
            _ => Err("IMAP/SMTP protocol not configured".to_string()),
        }
    }

    fn smtp_params(&self) -> Result<(&str, u16, &MailSecurity), String> {
        match &self.account.mail_protocol {
            MailProtocol::ImapSmtp {
                smtp_server,
                smtp_port,
                security,
                ..
            } => Ok((smtp_server.as_str(), *smtp_port, security)),
            _ => Err("IMAP/SMTP protocol not configured".to_string()),
        }
    }

    async fn imap_session(&self) -> Result<async_imap::Session<ImapConnection>, String> {
        let (server, port, security) = self.imap_params()?;
        let domain = tokio_rustls::rustls::pki_types::ServerName::try_from(server)
            .map_err(|e| format!("invalid IMAP server name: {}", e))?
            .to_owned();
        let tls_connector = tls_connector();

        let client = match security {
            MailSecurity::Tls => {
                let tcp = TcpStream::connect((server, port))
                    .await
                    .map_err(|e| format!("IMAP connect failed: {}", e))?;
                let tls = tls_connector
                    .connect(domain, tcp)
                    .await
                    .map_err(|e| format!("IMAP TLS handshake failed: {}", e))?;
                let mut client = async_imap::Client::new(ImapConnection::Tls(Box::new(tls)));
                let _ = client.read_response().await;
                client
            }
            MailSecurity::StartTls => {
                let tcp = TcpStream::connect((server, port))
                    .await
                    .map_err(|e| format!("IMAP connect failed: {}", e))?;
                let mut client = async_imap::Client::new(ImapConnection::Plain(Box::new(tcp)));
                let _ = client.read_response().await;
                client
                    .run_command_and_check_ok("STARTTLS", None)
                    .await
                    .map_err(|e| format!("IMAP STARTTLS failed: {}", e))?;
                let plain = match client.into_inner() {
                    ImapConnection::Plain(s) => *s,
                    _ => unreachable!(),
                };
                let tls = tls_connector
                    .connect(domain, plain)
                    .await
                    .map_err(|e| format!("IMAP TLS handshake failed: {}", e))?;
                async_imap::Client::new(ImapConnection::Tls(Box::new(tls)))
            }
            MailSecurity::None => {
                let tcp = TcpStream::connect((server, port))
                    .await
                    .map_err(|e| format!("IMAP connect failed: {}", e))?;
                let mut client = async_imap::Client::new(ImapConnection::Plain(Box::new(tcp)));
                let _ = client.read_response().await;
                client
            }
        };

        client
            .login(&self.account.username, &self.account.password)
            .await
            .map_err(|(e, _)| format!("IMAP login failed: {}", e))
    }

    pub async fn fetch_mailboxes(&mut self) -> Result<Vec<Mailbox>, String> {
        let mut session = self.imap_session().await?;
        let account_id = format!("{}@{}", self.account.username, self.account.server_url);

        let names: Vec<_> = session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| format!("IMAP LIST failed: {}", e))?
            .try_collect()
            .await
            .map_err(|e| format!("IMAP LIST failed: {}", e))?;

        let mut mailboxes = Vec::new();
        for name in names {
            let mbox_name = name.name().to_string();
            if mbox_name.is_empty() {
                continue;
            }

            let (total, unread) = match session.status(&mbox_name, "(MESSAGES UNSEEN)").await {
                Ok(status) => (status.exists, status.unseen.unwrap_or(0)),
                Err(_) => (0, 0),
            };

            let role = infer_role(&mbox_name);
            mailboxes.push(Mailbox {
                id: mbox_name.clone(),
                account_id: account_id.clone(),
                name: mbox_name,
                role,
                sort_order: 0,
                total_emails: total as i32,
                unread_emails: unread as i32,
            });
        }

        mailboxes.sort_by_key(|a| a.name.to_lowercase());
        Ok(mailboxes)
    }

    pub async fn fetch_emails(&mut self, mailbox_id: &str) -> Result<Vec<Email>, String> {
        let mut session = self.imap_session().await?;
        let account_id = format!("{}@{}", self.account.username, self.account.server_url);

        session
            .select(mailbox_id)
            .await
            .map_err(|e| format!("IMAP SELECT failed: {}", e))?;

        let uids: Vec<u32> = {
            let set = session
                .uid_search("ALL")
                .await
                .map_err(|e| format!("IMAP SEARCH failed: {}", e))?;
            let mut ids: Vec<_> = set.into_iter().collect();
            ids.sort_unstable_by(|a, b| b.cmp(a));
            ids.into_iter().take(50).collect()
        };

        let mut emails = Vec::new();
        for uid in uids {
            if let Some(email) = fetch_email_by_uid(&mut session, uid, mailbox_id, &account_id).await {
                emails.push(email);
            }
        }

        Ok(emails)
    }

    pub async fn fetch_email_body(&mut self, email_id: &str) -> Result<Email, String> {
        let (uid, mailbox_id) = parse_email_id(email_id)?;
        let mut session = self.imap_session().await?;
        let account_id = format!("{}@{}", self.account.username, self.account.server_url);

        session
            .select(&mailbox_id)
            .await
            .map_err(|e| format!("IMAP SELECT failed: {}", e))?;

        fetch_email_by_uid(&mut session, uid, &mailbox_id, &account_id)
            .await
            .ok_or_else(|| "Email not found".to_string())
    }

    pub async fn mark_email_read(&mut self, email_id: &str, read: bool) -> Result<(), String> {
        let (uid, mailbox_id) = parse_email_id(email_id)?;
        let mut session = self.imap_session().await?;

        session
            .select(&mailbox_id)
            .await
            .map_err(|e| format!("IMAP SELECT failed: {}", e))?;

        let query = if read {
            "+FLAGS (\\Seen)"
        } else {
            "-FLAGS (\\Seen)"
        };
        session
            .uid_store(uid.to_string(), query)
            .await
            .map_err(|e| format!("IMAP STORE failed: {}", e))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| format!("IMAP STORE failed: {}", e))?;

        Ok(())
    }

    pub async fn delete_email(&mut self, email_id: &str) -> Result<(), String> {
        let (uid, mailbox_id) = parse_email_id(email_id)?;
        let mut session = self.imap_session().await?;

        session
            .select(&mailbox_id)
            .await
            .map_err(|e| format!("IMAP SELECT failed: {}", e))?;

        session
            .uid_store(uid.to_string(), "+FLAGS (\\Deleted)")
            .await
            .map_err(|e| format!("IMAP STORE failed: {}", e))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| format!("IMAP STORE failed: {}", e))?;

        session
            .expunge()
            .await
            .map_err(|e| format!("IMAP EXPUNGE failed: {}", e))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| format!("IMAP EXPUNGE failed: {}", e))?;

        Ok(())
    }

    pub async fn send_email(
        &mut self,
        to: &[String],
        cc: &[String],
        bcc: &[String],
        subject: &str,
        body_text: &str,
        body_html: Option<&str>,
    ) -> Result<(), String> {
        let (server, port, security) = self.smtp_params()?;
        let from: SmtpMailbox = self
            .account
            .username
            .parse()
            .map_err(|e| format!("invalid From address: {:?}", e))?;

        let mut builder = Message::builder()
            .from(from.clone())
            .subject(subject.to_string());

        for addr in to {
            let mailbox: SmtpMailbox = addr
                .parse()
                .map_err(|e| format!("invalid To address {}: {:?}", addr, e))?;
            builder = builder.to(mailbox);
        }
        for addr in cc {
            let mailbox: SmtpMailbox = addr
                .parse()
                .map_err(|e| format!("invalid CC address {}: {:?}", addr, e))?;
            builder = builder.cc(mailbox);
        }
        for addr in bcc {
            let mailbox: SmtpMailbox = addr
                .parse()
                .map_err(|e| format!("invalid BCC address {}: {:?}", addr, e))?;
            builder = builder.bcc(mailbox);
        }

        let message = if let Some(html) = body_html {
            builder.multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(body_text.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html.to_string()),
                    ),
            )
        } else {
            builder.body(body_text.to_string())
        }
        .map_err(|e| format!("failed to build message: {}", e))?;

        let creds = Credentials::new(self.account.username.clone(), self.account.password.clone());
        let transport = match security {
            MailSecurity::Tls => {
                let tls_params = TlsParameters::new(server.to_string())
                    .map_err(|e| format!("SMTP TLS parameters failed: {}", e))?;
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(server)
                    .port(port)
                    .tls(Tls::Wrapper(tls_params))
                    .credentials(creds)
                    .build()
            }
            MailSecurity::StartTls => AsyncSmtpTransport::<Tokio1Executor>::relay(server)
                .map_err(|e| format!("SMTP relay setup failed: {}", e))?
                .port(port)
                .credentials(creds)
                .build(),
            MailSecurity::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(server)
                .port(port)
                .tls(Tls::None)
                .credentials(creds)
                .build(),
        };

        transport
            .send(message)
            .await
            .map_err(|e| format!("SMTP send failed: {}", e))?;

        Ok(())
    }
}

fn tls_connector() -> tokio_rustls::TlsConnector {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

fn infer_role(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if lower == "inbox" {
        Some("inbox".to_string())
    } else if lower.contains("sent") {
        Some("sent".to_string())
    } else if lower.contains("draft") {
        Some("drafts".to_string())
    } else if lower.contains("trash") || lower.contains("deleted") || lower.contains("bin") {
        Some("trash".to_string())
    } else if lower.contains("spam") || lower.contains("junk") {
        Some("spam".to_string())
    } else if lower.contains("archive") {
        Some("archive".to_string())
    } else {
        None
    }
}

fn parse_email_id(id: &str) -> Result<(u32, String), String> {
    let mut parts = id.splitn(2, '\x1f');
    let uid = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(|| "invalid email id".to_string())?;
    let mailbox = parts
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| "invalid email id".to_string())?;
    Ok((uid, mailbox))
}

fn email_id(uid: u32, mailbox_id: &str) -> String {
    format!("{}\x1f{}", uid, mailbox_id)
}

async fn fetch_email_by_uid(
    session: &mut async_imap::Session<ImapConnection>,
    uid: u32,
    mailbox_id: &str,
    account_id: &str,
) -> Option<Email> {
    let fetches: Vec<_> = match session
        .uid_fetch(uid.to_string(), "RFC822 FLAGS")
        .await
    {
        Ok(stream) => match stream.try_collect().await {
            Ok(v) => v,
            Err(_) => return None,
        },
        Err(_) => return None,
    };

    let fetch = fetches.first()?;
    let bytes = fetch.body()?;
    let flags: Vec<Flag> = fetch.flags().collect();
    build_email(uid, mailbox_id, account_id, bytes, &flags)
}

fn build_email(
    uid: u32,
    mailbox_id: &str,
    account_id: &str,
    bytes: &[u8],
    flags: &[Flag],
) -> Option<Email> {
    let message = mail_parser::MessageParser::default().parse(bytes)?;

    let from = parse_addresses(message.from());
    let to = parse_addresses(message.to());
    let cc = parse_addresses(message.cc());
    let bcc = parse_addresses(message.bcc());

    let subject = message.subject().unwrap_or_default().to_string();
    let received_at = message
        .date()
        .and_then(parse_datetime)
        .unwrap_or_else(Utc::now);

    let body_text = message.body_text(0).map(|s| s.to_string());
    let body_html = message.body_html(0).map(|s| s.to_string());
    let preview = message
        .body_preview(200)
        .map(|s| s.to_string())
        .or_else(|| body_text.as_ref().map(|s| s.chars().take(200).collect()))
        .unwrap_or_default();

    let mut keywords = Vec::new();
    if flags.iter().any(|f| matches!(f, Flag::Seen)) {
        keywords.push("$seen".to_string());
    }

    Some(Email {
        id: email_id(uid, mailbox_id),
        account_id: account_id.to_string(),
        thread_id: uid.to_string(),
        mailbox_ids: vec![mailbox_id.to_string()],
        from,
        to,
        cc,
        bcc,
        subject,
        received_at,
        preview,
        body_text,
        body_html,
        keywords,
        has_attachments: !message.attachments.is_empty(),
        size: bytes.len() as i64,
    })
}

fn parse_addresses(value: Option<&ParserAddress<'_>>) -> Vec<EmailAddress> {
    let addresses = match value {
        Some(a) => a,
        _ => return Vec::new(),
    };

    let addrs: Vec<&mail_parser::Addr<'_>> = match addresses {
        ParserAddress::List(list) => list.iter().collect(),
        ParserAddress::Group(groups) => groups.iter().flat_map(|g| &g.addresses).collect(),
    };

    addrs
        .into_iter()
        .map(|a| EmailAddress {
            name: a.name.as_ref().map(|n| n.to_string()),
            email: a.address.as_ref().map(|e| e.to_string()).unwrap_or_default(),
        })
        .collect()
}

fn parse_datetime(value: &mail_parser::DateTime) -> Option<DateTime<Utc>> {
    let s = value.to_rfc3339();
    DateTime::parse_from_rfc3339(&s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_imap::types::Flag;

    fn sample_message_bytes() -> Vec<u8> {
        "From: Alice <alice@example.com>\r\n\
         To: Bob <bob@example.com>\r\n\
         Cc: Carol <carol@example.com>\r\n\
         Subject: Test message\r\n\
         Date: Mon, 15 Jun 2026 10:00:00 +0000\r\n\
         Content-Type: text/plain\r\n\
         Message-Id: <msg1@example.com>\r\n\r\n\
         Hello, world!"
            .bytes()
            .collect()
    }

    #[test]
    fn infer_role_recognises_standard_mailboxes() {
        assert_eq!(infer_role("INBOX"), Some("inbox".to_string()));
        assert_eq!(infer_role("Sent"), Some("sent".to_string()));
        assert_eq!(infer_role("Drafts"), Some("drafts".to_string()));
        assert_eq!(infer_role("Trash"), Some("trash".to_string()));
        assert_eq!(infer_role("Deleted Items"), Some("trash".to_string()));
        assert_eq!(infer_role("Junk"), Some("spam".to_string()));
        assert_eq!(infer_role("Archive"), Some("archive".to_string()));
        assert_eq!(infer_role("Custom"), None);
    }

    #[test]
    fn parse_email_id_roundtrips_with_email_id() {
        assert_eq!(parse_email_id("123\x1fINBOX"), Ok((123, "INBOX".to_string())));
        assert_eq!(email_id(123, "INBOX"), "123\x1fINBOX");
        assert!(parse_email_id("abc\x1fINBOX").is_err());
        assert!(parse_email_id("123").is_err());
    }

    #[test]
    fn parse_addresses_from_message() {
        let bytes = sample_message_bytes();
        let message = mail_parser::MessageParser::default().parse(&bytes).unwrap();
        let from = parse_addresses(message.from());
        assert_eq!(from.len(), 1);
        assert_eq!(from[0].name, Some("Alice".to_string()));
        assert_eq!(from[0].email, "alice@example.com".to_string());

        let to = parse_addresses(message.to());
        assert_eq!(to.len(), 1);
        assert_eq!(to[0].email, "bob@example.com".to_string());

        let cc = parse_addresses(message.cc());
        assert_eq!(cc.len(), 1);
        assert_eq!(cc[0].email, "carol@example.com".to_string());

        assert!(parse_addresses(None).is_empty());
    }

    #[test]
    fn parse_datetime_from_message_date() {
        let bytes = sample_message_bytes();
        let message = mail_parser::MessageParser::default().parse(&bytes).unwrap();
        let dt = parse_datetime(message.date().unwrap()).unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-06-15T10:00:00+00:00");
    }

    #[test]
    fn build_email_from_parsed_message() {
        let bytes = sample_message_bytes();
        let email = build_email(42, "INBOX", "alice@example.com", &bytes, &[]).unwrap();
        assert_eq!(email.id, "42\x1fINBOX");
        assert_eq!(email.mailbox_ids, vec!["INBOX".to_string()]);
        assert_eq!(email.from.len(), 1);
        assert_eq!(email.subject, "Test message");
        assert_eq!(email.body_text, Some("Hello, world!".to_string()));
        assert!(!email.has_attachments);
        assert_eq!(email.size, bytes.len() as i64);
    }

    #[test]
    fn build_email_flags_mark_seen() {
        let bytes = sample_message_bytes();
        let email = build_email(1, "INBOX", "a", &bytes, &[Flag::Seen]).unwrap();
        assert!(email.keywords.contains(&"$seen".to_string()));
    }
}
