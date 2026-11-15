use std::collections::HashMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    models::account::{MailAccount, ServerAccount},
    personal_data::auth_header_for_account,
};

pub struct WebmailClient {
    client: reqwest::Client,
    base_url: String,
    authorization: String,
    account_id: String,
    sieve_account_id: String,
}

impl WebmailClient {
    pub fn new(account: &MailAccount) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: account.server_url().trim_end_matches('/').to_owned(),
            authorization: auth_header_for_account(account),
            account_id: account.id().to_owned(),
            sieve_account_id: account.id().to_owned(),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub async fn discover(&mut self) -> Result<(), String> {
        let session = self.session().await?;
        self.account_id = session.account_id()?.to_owned();
        self.sieve_account_id = session.sieve_account_id()?.to_owned();
        Ok(())
    }

    pub async fn sieve_scripts(&self) -> Result<Vec<(String, bool)>, String> {
        let result = self
            .request_sieve(json!([["SieveScript/get", {
                "accountId": self.sieve_account_id,
            }, "scripts"]]))
            .await?;
        let scripts: Vec<SieveScript> = response_list(&result, "scripts")?;
        Ok(scripts
            .into_iter()
            .filter_map(|script| script.name.map(|name| (name, script.is_active)))
            .collect())
    }

    pub async fn sieve_script(&self, name: &str) -> Result<String, String> {
        let result = self
            .request_sieve(json!([
                ["SieveScript/get", { "accountId": self.sieve_account_id }, "scripts"],
                ["Blob/get", {
                    "accountId": self.sieve_account_id,
                    "#ids": {
                        "resultOf": "scripts",
                        "name": "SieveScript/get",
                        "path": "/list/*/blobId"
                    },
                    "properties": ["id", "data:asText"]
                }, "blobs"]
            ]))
            .await?;
        let scripts: Vec<SieveScript> = response_list(&result, "scripts")?;
        let script = scripts
            .iter()
            .find(|script| script.name.as_deref() == Some(name))
            .ok_or_else(|| format!("Sieve script {name:?} was not found"))?;
        let blob_id = script.blob_id.as_deref().ok_or("Sieve script has no blob")?;
        let blobs: Vec<SieveBlob> = response_list(&result, "blobs")?;
        blobs
            .into_iter()
            .find(|blob| blob.id == blob_id)
            .and_then(|blob| blob.data_as_text)
            .ok_or_else(|| format!("Sieve script {name:?} content was not returned"))
    }

    pub async fn save_sieve_script(
        &self,
        original_name: &str,
        name: &str,
        content: &str,
        activate: bool,
    ) -> Result<(), String> {
        if content.is_empty() {
            return Err("Sieve scripts must not be empty".to_owned());
        }
        let scripts = self.sieve_script_metadata().await?;
        let existing = if original_name.is_empty() {
            None
        } else {
            scripts
                .iter()
                .find(|script| script.name.as_deref() == Some(original_name))
        };
        let blob_id = self.upload_sieve(content).await?;
        let mut args = json!({ "accountId": self.sieve_account_id });
        if let Some(script) = existing {
            let id = script.id.as_deref().ok_or("Sieve script has no id")?;
            args["update"] = json!({ (id): { "name": name, "blobId": blob_id } });
            if activate {
                args["onSuccessActivateScript"] = json!(id);
            }
        } else {
            args["create"] = json!({ "newScript": { "name": name, "blobId": blob_id } });
            if activate {
                args["onSuccessActivateScript"] = json!("#newScript");
            }
        }
        let result = self
            .request_sieve(json!([["SieveScript/set", args, "save"]]))
            .await?;
        check_set_errors(&result, "save")
    }

    pub async fn delete_sieve_script(&self, name: &str) -> Result<(), String> {
        let scripts = self.sieve_script_metadata().await?;
        let script = scripts
            .iter()
            .find(|script| script.name.as_deref() == Some(name))
            .ok_or_else(|| format!("Sieve script {name:?} was not found"))?;
        let id = script.id.as_deref().ok_or("Sieve script has no id")?;
        if script.is_active {
            let result = self
                .request_sieve(json!([["SieveScript/set", {
                    "accountId": self.sieve_account_id,
                    "onSuccessDeactivateScript": true
                }, "deactivate"]]))
                .await?;
            check_set_errors(&result, "deactivate")?;
        }
        let result = self
            .request_sieve(json!([["SieveScript/set", {
                "accountId": self.sieve_account_id,
                "destroy": [id]
            }, "delete"]]))
            .await?;
        check_set_errors(&result, "delete")
    }

    pub async fn set_active_sieve_script(&self, name: &str, active: bool) -> Result<(), String> {
        let scripts = self.sieve_script_metadata().await?;
        let script = scripts
            .iter()
            .find(|script| script.name.as_deref() == Some(name))
            .ok_or_else(|| format!("Sieve script {name:?} was not found"))?;
        let mut args = json!({ "accountId": self.sieve_account_id });
        if active {
            args["onSuccessActivateScript"] = json!(script.id.as_deref().ok_or("Sieve script has no id")?);
        } else {
            args["onSuccessDeactivateScript"] = json!(true);
        }
        let result = self
            .request_sieve(json!([["SieveScript/set", args, "toggle"]]))
            .await?;
        check_set_errors(&result, "toggle")
    }

    async fn sieve_script_metadata(&self) -> Result<Vec<SieveScript>, String> {
        let result = self
            .request_sieve(json!([["SieveScript/get", {
                "accountId": self.sieve_account_id,
            }, "scripts"]]))
            .await?;
        response_list(&result, "scripts")
    }

    async fn upload_sieve(&self, content: &str) -> Result<String, String> {
        let mut url = reqwest::Url::parse(&format!("{}/jmap/upload", self.base_url))
            .map_err(|error| error.to_string())?;
        url.path_segments_mut()
            .map_err(|_| "invalid Stalwart URL".to_owned())?
            .push(&self.sieve_account_id)
            .push("");
        let response = self
            .client
            .post(url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/sieve")
            .body(content.to_owned())
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        response
            .json::<UploadedBlob>()
            .await
            .map(|blob| blob.blob_id)
            .map_err(|error| error.to_string())
    }

    async fn request_sieve(&self, method_calls: Value) -> Result<Value, String> {
        self.request_with_capabilities(
            method_calls,
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:sieve", "urn:ietf:params:jmap:blob"],
        )
        .await
    }

    pub async fn session(&self) -> Result<Session, String> {
        let response = self
            .client
            .get(format!("{}/jmap/session", self.base_url))
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        response.json().await.map_err(|error| error.to_string())
    }

    pub async fn mailboxes(&self) -> Result<Vec<Mailbox>, String> {
        let result = self.request(json!([["Mailbox/get", { "accountId": self.account_id, "ids": null }, "mailboxes"]])).await?;
        let mut mailboxes: Vec<Mailbox> = response_list(&result, "mailboxes")?;
        mailboxes.sort_by(|left, right| {
            role_order(left.role.as_deref())
                .cmp(&role_order(right.role.as_deref()))
                .then(left.sort_order.cmp(&right.sort_order))
                .then(left.name.cmp(&right.name))
        });
        Ok(mailboxes)
    }

    pub async fn emails(&self, mailbox_id: &str) -> Result<Vec<Email>, String> {
        let result = self.request(json!([
            ["Email/query", {
                "accountId": self.account_id,
                "filter": { "inMailbox": mailbox_id },
                "sort": [{ "property": "receivedAt", "isAscending": false }],
                "limit": 50,
                "position": 0,
            }, "emailIds"],
            ["Email/get", {
                "accountId": self.account_id,
                "#ids": { "resultOf": "emailIds", "name": "Email/query", "path": "/ids" },
                "properties": ["id", "threadId", "mailboxIds", "keywords", "from", "to", "subject", "receivedAt", "preview", "hasAttachment", "size"],
            }, "emails"],
        ])).await?;
        response_list(&result, "emails")
    }

    pub async fn email(&self, email_id: &str) -> Result<Option<Email>, String> {
        let result = self.request(json!([["Email/get", {
            "accountId": self.account_id,
            "ids": [email_id],
            "properties": [
                "id", "blobId", "mailboxIds", "keywords", "from", "to", "cc", "replyTo", "inReplyTo", "references",
                "subject", "receivedAt", "preview", "hasAttachment", "size", "bodyValues", "textBody", "htmlBody", "attachments",
            ],
            "fetchAllBodyValues": true,
            "maxBodyValueBytes": 2 * 1024 * 1024,
        }, "email"]])).await?;
        let mut emails: Vec<Email> = response_list(&result, "email")?;
        let Some(mut email) = emails.pop() else {
            return Ok(None);
        };
        if let Some(blob_id) = email.blob_id.as_deref() {
            let download = self.download(blob_id, "message.eml").await?;
            if (200..300).contains(&download.status)
                && let Some(message) = mail_parser::MessageParser::default().parse(&download.body)
            {
                email.mime_parsed = true;
                email.parsed_text_body = message.body_text(0).map(|body| body.into_owned());
                email.parsed_html_body = message.body_html(0).map(|body| body.into_owned());
            }
        }
        Ok(Some(email))
    }

    pub async fn identities(&self) -> Result<Vec<Identity>, String> {
        let result = self.request(json!([["Identity/get", { "accountId": self.account_id, "ids": null }, "identities"]])).await?;
        response_list(&result, "identities")
    }

    pub async fn mark_read(&self, email_id: &str) -> Result<(), String> {
        self.update(json!([["Email/set", {
            "accountId": self.account_id,
            "update": { (email_id): { "keywords/$seen": true } },
        }, "update"]]))
            .await
    }

    pub async fn move_email(&self, email_id: &str, mailbox_id: &str) -> Result<(), String> {
        self.update(json!([["Email/set", {
            "accountId": self.account_id,
            "update": { (email_id): { "mailboxIds": { (mailbox_id): true } } },
        }, "move"]]))
            .await
    }

    pub async fn delete_email(&self, email_id: &str, trash_id: Option<&str>) -> Result<(), String> {
        if let Some(trash_id) = trash_id {
            return self.move_email(email_id, trash_id).await;
        }
        self.update(json!([["Email/set", {
            "accountId": self.account_id,
            "destroy": [email_id],
        }, "destroy"]]))
            .await
    }

    pub async fn delete_mailbox(&self, mailbox_id: &str) -> Result<(), String> {
        self.update(json!([["Mailbox/set", {
            "accountId": self.account_id,
            "destroy": [mailbox_id],
        }, "destroyMailbox"]]))
            .await
    }

    pub async fn send(&self, outgoing: OutgoingEmail) -> Result<(), String> {
        let mailboxes = self.mailboxes().await?;
        let sent = mailboxes
            .iter()
            .find(|mailbox| mailbox.role.as_deref() == Some("sent"));
        let from = outgoing.from.email.clone();
        let mut create = json!({
            "from": [outgoing.from],
            "to": outgoing.to,
            "cc": outgoing.cc,
            "bcc": outgoing.bcc,
            "subject": outgoing.subject,
            "keywords": { "$seen": true },
            "mailboxIds": sent.map_or_else(|| json!({}), |mailbox| json!({ (mailbox.id.clone()): true })),
            "bodyValues": { "text": { "value": outgoing.text_body } },
            "textBody": [{ "partId": "text", "type": "text/plain" }],
            "inReplyTo": outgoing.in_reply_to,
            "references": outgoing.references,
        });
        if !outgoing.attachments.is_empty() {
            let mut attachments = Vec::with_capacity(outgoing.attachments.len());
            for attachment in outgoing.attachments {
                let uploaded = self.upload(&attachment).await?;
                attachments.push(json!({
                    "blobId": uploaded.blob_id,
                    "type": attachment.content_type,
                    "name": attachment.name,
                    "size": attachment.data.len(),
                    "disposition": "attachment",
                }));
            }
            create["attachments"] = json!(attachments);
        }
        let recipients: Vec<_> = create["to"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(create["cc"].as_array().into_iter().flatten())
            .chain(create["bcc"].as_array().into_iter().flatten())
            .filter_map(|address| address.get("email").cloned())
            .map(|address| json!({ "email": address }))
            .collect();
        let result = self.request(json!([
            ["Email/set", { "accountId": self.account_id, "create": { "draft": create } }, "createEmail"],
            ["EmailSubmission/set", {
                "accountId": self.account_id,
                "create": {
                    "send": {
                        "emailId": "#draft",
                        "identityId": outgoing.identity_id,
                        "envelope": { "mailFrom": { "email": from }, "rcptTo": recipients },
                    },
                },
            }, "submit"],
        ])).await?;
        check_not_created(&result, "createEmail")?;
        check_not_created(&result, "submit")
    }

    pub async fn download(&self, blob_id: &str, name: &str) -> Result<Download, String> {
        let mut url = reqwest::Url::parse(&format!("{}/jmap/download", self.base_url))
            .map_err(|error| error.to_string())?;
        url.path_segments_mut()
            .map_err(|_| "invalid Stalwart URL".to_owned())?
            .extend([self.account_id.as_str(), blob_id, name]);
        let response = self
            .client
            .get(url)
            .header("Authorization", &self.authorization)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                (!matches!(
                    name.as_str(),
                    "content-length" | "content-encoding" | "transfer-encoding"
                ))
                .then(|| Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned())))
            })
            .flatten()
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| error.to_string())?
            .to_vec();
        Ok(Download {
            status,
            headers,
            body,
        })
    }

    async fn update(&self, calls: Value) -> Result<(), String> {
        let result = self.request(calls).await?;
        for response in result
            .get("methodResponses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(body) = response.get(1).and_then(Value::as_object) {
                for field in ["notUpdated", "notDestroyed"] {
                    if body
                        .get(field)
                        .and_then(Value::as_object)
                        .is_some_and(|errors| !errors.is_empty())
                    {
                        return Err(Value::Object(body.clone()).to_string());
                    }
                }
            }
        }
        Ok(())
    }

    async fn request(&self, method_calls: Value) -> Result<Value, String> {
        self.request_with_capabilities(
            method_calls,
            &[
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:submission",
            ],
        )
        .await
    }

    async fn request_with_capabilities(
        &self,
        method_calls: Value,
        capabilities: &[&str],
    ) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}/jmap", self.base_url))
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json")
            .json(&json!({
                "using": capabilities,
                "methodCalls": method_calls,
            }))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        let result: Value = response.json().await.map_err(|error| error.to_string())?;
        if let Some(responses) = result.get("methodResponses").and_then(Value::as_array) {
            for response in responses {
                if response.get(0).and_then(Value::as_str) == Some("error") {
                    let description = response
                        .get(1)
                        .and_then(|error| error.get("description"))
                        .and_then(Value::as_str)
                        .unwrap_or("JMAP request failed");
                    return Err(description.to_owned());
                }
            }
        }
        Ok(result)
    }

    async fn upload(&self, attachment: &OutgoingAttachment) -> Result<UploadedBlob, String> {
        let mut url = reqwest::Url::parse(&format!("{}/jmap/upload", self.base_url))
            .map_err(|error| error.to_string())?;
        url.path_segments_mut()
            .map_err(|_| "invalid Stalwart URL".to_owned())?
            .push(&self.account_id);
        let response = self
            .client
            .post(url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", &attachment.content_type)
            .body(attachment.data.clone())
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        response.json().await.map_err(|error| error.to_string())
    }
}

#[derive(Deserialize)]
pub struct Session {
    #[serde(rename = "primaryAccounts")]
    pub primary_accounts: HashMap<String, String>,
    pub accounts: HashMap<String, Value>,
}

impl Session {
    pub fn account_id(&self) -> Result<&str, String> {
        self.primary_accounts
            .get("urn:ietf:params:jmap:mail")
            .or_else(|| self.accounts.keys().next())
            .map(String::as_str)
            .ok_or_else(|| "JMAP session has no mail account".to_owned())
    }

    pub fn sieve_account_id(&self) -> Result<&str, String> {
        self.primary_accounts
            .get("urn:ietf:params:jmap:sieve")
            .or_else(|| self.primary_accounts.get("urn:ietf:params:jmap:mail"))
            .or_else(|| self.accounts.keys().next())
            .map(String::as_str)
            .ok_or_else(|| "JMAP session has no Sieve account".to_owned())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SieveScript {
    id: Option<String>,
    name: Option<String>,
    blob_id: Option<String>,
    #[serde(default)]
    is_active: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SieveBlob {
    id: String,
    #[serde(rename = "data:asText")]
    data_as_text: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub total_emails: u64,
    pub unread_emails: u64,
    pub sort_order: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyPart {
    pub part_id: Option<String>,
    pub blob_id: Option<String>,
    pub size: Option<u64>,
    #[serde(rename = "type")]
    pub content_type: String,
    pub name: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct BodyValue {
    pub value: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    pub id: String,
    pub blob_id: Option<String>,
    pub thread_id: Option<String>,
    pub mailbox_ids: HashMap<String, bool>,
    pub keywords: HashMap<String, bool>,
    pub size: u64,
    pub received_at: String,
    pub from: Option<Vec<EmailAddress>>,
    pub to: Option<Vec<EmailAddress>>,
    pub cc: Option<Vec<EmailAddress>>,
    pub reply_to: Option<Vec<EmailAddress>>,
    pub in_reply_to: Option<Vec<String>>,
    pub references: Option<Vec<String>>,
    pub subject: Option<String>,
    pub preview: Option<String>,
    pub has_attachment: Option<bool>,
    pub body_values: Option<HashMap<String, BodyValue>>,
    pub text_body: Option<Vec<BodyPart>>,
    pub html_body: Option<Vec<BodyPart>>,
    pub attachments: Option<Vec<BodyPart>>,
    #[serde(skip)]
    parsed_text_body: Option<String>,
    #[serde(skip)]
    parsed_html_body: Option<String>,
    #[serde(skip)]
    mime_parsed: bool,
}

impl Email {
    pub fn sender(&self) -> String {
        self.from
            .as_ref()
            .and_then(|addresses| addresses.first())
            .map_or_else(
                || "Unknown".to_owned(),
                |address| {
                    address
                        .name
                        .clone()
                        .unwrap_or_else(|| address.email.clone())
                },
            )
    }

    pub fn body(&self, html: bool) -> Option<&str> {
        let parsed_body = if html {
            self.parsed_html_body.as_deref()
        } else {
            self.parsed_text_body.as_deref()
        };
        if self.mime_parsed {
            return parsed_body;
        }
        let parts = if html {
            &self.html_body
        } else {
            &self.text_body
        };
        let part_id = parts.as_ref()?.first()?.part_id.as_ref()?;
        Some(self.body_values.as_ref()?.get(part_id)?.value.as_str())
    }
}

#[derive(Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub email: String,
}

pub struct OutgoingAttachment {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

pub struct OutgoingEmail {
    pub identity_id: String,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub bcc: Vec<EmailAddress>,
    pub subject: String,
    pub text_body: String,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub attachments: Vec<OutgoingAttachment>,
}

pub struct Download {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadedBlob {
    blob_id: String,
}

fn response_list<T: DeserializeOwned>(result: &Value, tag: &str) -> Result<Vec<T>, String> {
    let response = result
        .get("methodResponses")
        .and_then(Value::as_array)
        .and_then(|responses| {
            responses
                .iter()
                .find(|response| response.get(2).and_then(Value::as_str) == Some(tag))
        })
        .ok_or_else(|| format!("missing JMAP response {tag}"))?;
    let list = response
        .get(1)
        .and_then(|body| body.get("list"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::from_value(list).map_err(|error| error.to_string())
}

fn check_not_created(result: &Value, tag: &str) -> Result<(), String> {
    let response = result
        .get("methodResponses")
        .and_then(Value::as_array)
        .and_then(|responses| {
            responses
                .iter()
                .find(|response| response.get(2).and_then(Value::as_str) == Some(tag))
        });
    if let Some(errors) = response
        .and_then(|response| response.get(1))
        .and_then(|body| body.get("notCreated"))
        .and_then(Value::as_object)
        .filter(|errors| !errors.is_empty())
    {
        return Err(Value::Object(errors.clone()).to_string());
    }
    Ok(())
}

fn check_set_errors(result: &Value, tag: &str) -> Result<(), String> {
    let response = result
        .get("methodResponses")
        .and_then(Value::as_array)
        .and_then(|responses| {
            responses
                .iter()
                .find(|response| response.get(2).and_then(Value::as_str) == Some(tag))
        })
        .ok_or_else(|| format!("missing JMAP response {tag}"))?;
    let body = response
        .get(1)
        .ok_or_else(|| format!("invalid JMAP response {tag}"))?;
    for field in ["notCreated", "notUpdated", "notDestroyed"] {
        if let Some(errors) = body
            .get(field)
            .and_then(Value::as_object)
            .filter(|errors| !errors.is_empty())
        {
            return Err(Value::Object(errors.clone()).to_string());
        }
    }
    Ok(())
}

fn role_order(role: Option<&str>) -> u8 {
    match role {
        Some("inbox") => 0,
        Some("sent") => 1,
        Some("drafts") => 2,
        Some("junk") => 3,
        Some("trash") => 4,
        Some("archive") => 5,
        _ => 10,
    }
}
