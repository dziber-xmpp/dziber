use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::models::account::MailAccount;
use crate::models::mail::{Email, EmailAddress, Mailbox};
use crate::personal_data::auth_header_for_account;

pub struct JmapMailClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: String,
    account_id: String,
    session: Option<JmapSession>,
}

#[derive(Debug, Clone, Deserialize)]
struct JmapSession {
    api_url: String,
}

impl JmapMailClient {
    pub fn new(account: &MailAccount) -> Self {
        let auth_header = auth_header_for_account(account);
        let base_url = account.server_url.trim_end_matches('/').to_string();
        let account_id = account.id.clone();
        Self {
            client: reqwest::Client::new(),
            base_url,
            auth_header,
            account_id,
            session: None,
        }
    }

    fn build_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
        }
    }

    fn default_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header).unwrap(),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }

    async fn jmap_request(&self, method_calls: Vec<Value>) -> Result<Value, String> {
        let url = self
            .session
            .as_ref()
            .map(|s| s.api_url.clone())
            .unwrap_or_else(|| self.build_url("/jmap"));

        let body = json!({
            "using": [
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:submission"
            ],
            "methodCalls": method_calls
        });

        let response = self
            .client
            .post(&url)
            .headers(self.default_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("JMAP request failed: HTTP {}", status));
        }

        response.json().await.map_err(|e| e.to_string())
    }

    fn extract_response<'a>(&self, data: &'a Value, method_name: &str) -> Option<&'a Value> {
        data.get("methodResponses")?
            .as_array()?
            .iter()
            .find(|item| {
                item.get(0)
                    .and_then(|v| v.as_str())
                    .map(|s| s == method_name)
                    .unwrap_or(false)
            })
            .and_then(|item| item.get(1))
    }

    pub async fn fetch_session(&mut self) -> Result<(), String> {
        let url = self.build_url("/jmap/session");
        let response = self
            .client
            .get(&url)
            .headers(self.default_headers())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("Failed to fetch JMAP session: HTTP {}", status));
        }

        let session: JmapSession = response.json().await.map_err(|e| e.to_string())?;
        self.session = Some(session);
        Ok(())
    }

    pub async fn fetch_mailboxes(&mut self) -> Result<Vec<Mailbox>, String> {
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let account_id = self.account_id.clone();
        let response = self
            .jmap_request(vec![json!([
                "Mailbox/get",
                {
                    "accountId": account_id,
                    "ids": null
                },
                "0"
            ])])
            .await?;

        let mut mailboxes = Vec::new();
        if let Some(args) = self.extract_response(&response, "Mailbox/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let role = item.get("role").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let sort_order = item.get("sortOrder").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let total = item.get("totalEmails").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let unread = item.get("unreadEmails").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                    mailboxes.push(Mailbox {
                        id,
                        account_id: self.account_id.clone(),
                        name,
                        role,
                        sort_order,
                        total_emails: total,
                        unread_emails: unread,
                    });
                }
            }

        Ok(mailboxes)
    }

    pub async fn fetch_emails(&mut self, mailbox_id: &str) -> Result<Vec<Email>, String> {
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let account_id = self.account_id.clone();
        let response = self
            .jmap_request(vec![
                json!([
                    "Email/query",
                    {
                        "accountId": account_id,
                        "filter": {
                            "inMailbox": mailbox_id
                        },
                        "sort": [{"property": "receivedAt", "isAscending": false}],
                        "limit": 50
                    },
                    "0"
                ]),
                json!([
                    "Email/get",
                    {
                        "accountId": account_id,
                        "#ids": {
                            "resultOf": "0",
                            "name": "Email/query",
                            "path": "/ids"
                        },
                        "properties": [
                            "id", "threadId", "mailboxIds", "keywords",
                            "from", "to", "cc", "bcc", "subject", "receivedAt",
                            "preview", "hasAttachment", "size"
                        ]
                    },
                    "1"
                ]),
            ])
            .await?;

        let mut emails = Vec::new();
        if let Some(args) = self.extract_response(&response, "Email/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    emails.push(self.parse_email(item));
                }
            }

        Ok(emails)
    }

    pub async fn fetch_email_body(&mut self, email_id: &str) -> Result<Email, String> {
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let account_id = self.account_id.clone();
        let response = self
            .jmap_request(vec![json!([
                "Email/get",
                {
                    "accountId": account_id,
                    "ids": [email_id],
                    "properties": ["bodyValues", "textBody", "htmlBody", "attachments"],
                    "fetchAllBodyValues": true,
                    "maxBodyValueBytes": 2097152
                },
                "0"
            ])])
            .await?;

        if let Some(args) = self.extract_response(&response, "Email/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array())
                && let Some(item) = list.first() {
                    let mut email = self.parse_email(item);

                    let body_values = item.get("bodyValues").and_then(|v| v.as_object());
                    if let Some(text_parts) = item.get("textBody").and_then(|v| v.as_array()) {
                        for part in text_parts {
                            let part_id = part.get("partId").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(value) = body_values
                                .and_then(|m| m.get(part_id))
                                .and_then(|v| v.get("value"))
                                .and_then(|v| v.as_str())
                            {
                                email.body_text = Some(value.to_string());
                                break;
                            }
                        }
                    }

                    if let Some(html_parts) = item.get("htmlBody").and_then(|v| v.as_array()) {
                        for part in html_parts {
                            let part_id = part.get("partId").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(value) = body_values
                                .and_then(|m| m.get(part_id))
                                .and_then(|v| v.get("value"))
                                .and_then(|v| v.as_str())
                            {
                                email.body_html = Some(value.to_string());
                                break;
                            }
                        }
                    }

                    return Ok(email);
                }

        Err("Email not found".to_string())
    }

    pub async fn set_email_keywords(
        &mut self,
        email_id: &str,
        keywords_map: Value,
    ) -> Result<(), String> {
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let account_id = self.account_id.clone();
        self.jmap_request(vec![json!([
            "Email/set",
            {
                "accountId": account_id,
                "update": {
                    email_id: keywords_map
                }
            },
            "0"
        ])])
        .await?;

        Ok(())
    }

    pub async fn mark_email_read(&mut self, email_id: &str, read: bool) -> Result<(), String> {
        let keywords_map = if read {
            json!({"keywords/$seen": true})
        } else {
            json!({"keywords/$seen": serde_json::Value::Null})
        };
        self.set_email_keywords(email_id, keywords_map).await
    }

    pub async fn delete_email(&mut self, email_id: &str) -> Result<(), String> {
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let account_id = self.account_id.clone();
        self.jmap_request(vec![json!([
            "Email/set",
            {
                "accountId": account_id,
                "destroy": [email_id]
            },
            "0"
        ])])
        .await?;

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
        if self.session.is_none() {
            self.fetch_session().await?;
        }

        let identity = self.fetch_default_identity().await?;
        let account_id = self.account_id.clone();

        let mut body_values = json!({
            "body-1": {
                "value": body_text,
                "type": "text/plain"
            }
        });

        if let Some(html) = body_html {
            body_values["body-2"] = json!({
                "value": html,
                "type": "text/html"
            });
        }

        let mut email_body_structure = json!([{
            "partId": "body-1",
            "type": "text/plain"
        }]);

        if body_html.is_some() {
            email_body_structure = json!([{
                "partId": "body-1",
                "type": "text/plain"
            }, {
                "partId": "body-2",
                "type": "text/html"
            }]);
        }

        let draft_id = uuid::Uuid::new_v4().to_string();
        let create_email = json!({
            "accountId": account_id,
            "create": {
                draft_id.clone(): {
                    "mailboxIds": {},
                    "keywords": { "$draft": true },
                    "from": [{ "email": identity.email, "name": identity.name }],
                    "to": to.iter().map(|e| json!({"email": e})).collect::<Vec<_>>(),
                    "cc": cc.iter().map(|e| json!({"email": e})).collect::<Vec<_>>(),
                    "bcc": bcc.iter().map(|e| json!({"email": e})).collect::<Vec<_>>(),
                    "subject": subject,
                    "bodyValues": body_values,
                    "textBody": [{
                        "partId": "body-1",
                        "type": "text/plain"
                    }],
                    "htmlBody": if body_html.is_some() {
                        Some(json!([{
                            "partId": "body-2",
                            "type": "text/html"
                        }]))
                    } else {
                        None::<Value>
                    },
                    "bodyStructure": email_body_structure
                }
            }
        });

        let response = self
            .jmap_request(vec![json!([
                "Email/set",
                create_email,
                "0"
            ])])
            .await?;

        let created_id = self
            .extract_response(&response, "Email/set")
            .and_then(|args| args.get("created"))
            .and_then(|created| created.get(&draft_id))
            .and_then(|item| item.get("id"))
            .and_then(|v| v.as_str())
            .ok_or("Failed to create draft email")?
            .to_string();

        let submission_id = uuid::Uuid::new_v4().to_string();
        self.jmap_request(vec![json!([
            "EmailSubmission/set",
            {
                "accountId": account_id,
                "create": {
                    submission_id: {
                        "emailId": created_id,
                        "identityId": identity.id,
                        "envelope": {
                            "mailFrom": { "email": identity.email },
                            "rcptTo": to.iter().chain(cc.iter()).chain(bcc.iter()).map(|e| json!({"email": e})).collect::<Vec<_>>()
                        }
                    }
                }
            },
            "0"
        ])])
        .await?;

        Ok(())
    }

    async fn fetch_default_identity(&mut self) -> Result<Identity, String> {
        let account_id = self.account_id.clone();
        let response = self
            .jmap_request(vec![json!([
                "Identity/get",
                {
                    "accountId": account_id,
                    "ids": null
                },
                "0"
            ])])
            .await?;

        if let Some(args) = self.extract_response(&response, "Identity/get")
            && let Some(list) = args.get("list").and_then(|v| v.as_array())
                && let Some(item) = list.first() {
                    return Ok(Identity {
                        id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        name: item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        email: item
                            .get("email")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    });
                }

        Err("No identity found".to_string())
    }

    fn parse_email(&self, item: &Value) -> Email {
        let parse_addresses = |key: &str| -> Vec<EmailAddress> {
            item.get(key)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|addr| EmailAddress {
                            name: addr.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            email: addr
                                .get("email")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        let mailbox_ids = item
            .get("mailboxIds")
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        let keywords = item
            .get("keywords")
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        let received_at = item
            .get("receivedAt")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Email {
            id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            account_id: self.account_id.clone(),
            thread_id: item
                .get("threadId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            mailbox_ids,
            from: parse_addresses("from"),
            to: parse_addresses("to"),
            cc: parse_addresses("cc"),
            bcc: parse_addresses("bcc"),
            subject: item
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            received_at,
            preview: item
                .get("preview")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            body_text: None,
            body_html: None,
            keywords,
            has_attachments: item.get("hasAttachment").and_then(|v| v.as_bool()).unwrap_or(false),
            size: item.get("size").and_then(|v| v.as_i64()).unwrap_or(0),
        }
    }
}

struct Identity {
    id: String,
    name: String,
    email: String,
}

pub enum MailClient {
    Jmap(JmapMailClient),
    ImapSmtp(crate::personal_data::imap_smtp::ImapSmtpClient),
}

impl MailClient {
    pub fn new(account: &MailAccount) -> Self {
        match &account.mail_protocol {
            crate::models::account::MailProtocol::Jmap => Self::Jmap(JmapMailClient::new(account)),
            crate::models::account::MailProtocol::ImapSmtp { .. } => {
                Self::ImapSmtp(crate::personal_data::imap_smtp::ImapSmtpClient::new(account))
            }
        }
    }

    pub async fn fetch_mailboxes(&mut self) -> Result<Vec<Mailbox>, String> {
        match self {
            Self::Jmap(c) => c.fetch_mailboxes().await,
            Self::ImapSmtp(c) => c.fetch_mailboxes().await,
        }
    }

    pub async fn fetch_emails(&mut self, mailbox_id: &str) -> Result<Vec<Email>, String> {
        match self {
            Self::Jmap(c) => c.fetch_emails(mailbox_id).await,
            Self::ImapSmtp(c) => c.fetch_emails(mailbox_id).await,
        }
    }

    pub async fn fetch_email_body(&mut self, email_id: &str) -> Result<Email, String> {
        match self {
            Self::Jmap(c) => c.fetch_email_body(email_id).await,
            Self::ImapSmtp(c) => c.fetch_email_body(email_id).await,
        }
    }

    pub async fn mark_email_read(&mut self, email_id: &str, read: bool) -> Result<(), String> {
        match self {
            Self::Jmap(c) => c.mark_email_read(email_id, read).await,
            Self::ImapSmtp(c) => c.mark_email_read(email_id, read).await,
        }
    }

    pub async fn delete_email(&mut self, email_id: &str) -> Result<(), String> {
        match self {
            Self::Jmap(c) => c.delete_email(email_id).await,
            Self::ImapSmtp(c) => c.delete_email(email_id).await,
        }
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
        match self {
            Self::Jmap(c) => c.send_email(to, cc, bcc, subject, body_text, body_html).await,
            Self::ImapSmtp(c) => c.send_email(to, cc, bcc, subject, body_text, body_html).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::account::{AuthMode, MailAccount, MailProtocol};

    fn test_account() -> MailAccount {
        MailAccount {
            id: "acc-1".to_string(),
            server_url: "https://example.com".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            auth_mode: AuthMode::Basic,
            mail_protocol: MailProtocol::Jmap,
            sieve_config: None,
        }
    }

    #[test]
    fn jmap_mail_client_new_sets_fields() {
        let account = test_account();
        let client = JmapMailClient::new(&account);
        assert_eq!(client.base_url, "https://example.com");
        assert_eq!(client.account_id, "acc-1");
        assert!(client.auth_header.starts_with("Basic "));
    }

    #[test]
    fn build_url_joins_or_passthroughs() {
        let client = JmapMailClient::new(&test_account());
        assert_eq!(client.build_url("/jmap"), "https://example.com/jmap");
        assert_eq!(client.build_url("jmap"), "https://example.com/jmap");
        assert_eq!(client.build_url("https://other.test/api"), "https://other.test/api");
    }

    #[test]
    fn extract_response_finds_method() {
        let client = JmapMailClient::new(&test_account());
        let data = json!({
            "methodResponses": [
                ["Mailbox/get", { "list": [] }, "0"],
                ["Email/get", { "list": [ { "id": "e1" } ] }, "1"]
            ]
        });
        let args = client.extract_response(&data, "Email/get").unwrap();
        assert_eq!(args["list"].as_array().unwrap().len(), 1);
        assert!(client.extract_response(&data, "Mailbox/changes").is_none());
    }

    #[test]
    fn parse_email_populates_fields() {
        let client = JmapMailClient::new(&test_account());
        let item = json!({
            "id": "e1",
            "threadId": "t1",
            "mailboxIds": { "m1": true },
            "keywords": { "$seen": true },
            "from": [{ "name": "Alice", "email": "alice@example.com" }],
            "to": [{ "email": "bob@example.com" }],
            "cc": [],
            "bcc": [],
            "subject": "Hello",
            "receivedAt": "2026-06-14T12:34:56Z",
            "preview": "Preview text",
            "hasAttachment": false,
            "size": 123
        });
        let email = client.parse_email(&item);
        assert_eq!(email.id, "e1");
        assert_eq!(email.account_id, "acc-1");
        assert_eq!(email.thread_id, "t1");
        assert_eq!(email.mailbox_ids, vec!["m1".to_string()]);
        assert_eq!(email.from.len(), 1);
        assert_eq!(email.from[0].name, Some("Alice".to_string()));
        assert_eq!(email.from[0].email, "alice@example.com".to_string());
        assert_eq!(email.to.len(), 1);
        assert_eq!(email.to[0].email, "bob@example.com".to_string());
        assert_eq!(email.subject, "Hello");
        assert_eq!(email.preview, "Preview text");
        assert!(email.keywords.contains(&"$seen".to_string()));
        assert_eq!(email.size, 123);
        assert!(!email.has_attachments);
    }

    #[test]
    fn parse_email_uses_defaults() {
        let client = JmapMailClient::new(&test_account());
        let item = json!({ "id": "e2" });
        let email = client.parse_email(&item);
        assert_eq!(email.id, "e2");
        assert!(email.subject.is_empty());
        assert!(email.from.is_empty());
        assert!(!email.has_attachments);
        assert_eq!(email.size, 0);
    }
}
