use serde_json::{Value, json};
use std::collections::HashMap;

use crate::models::account::ServerAccount;
use crate::personal_data::auth_header_for_account;

pub struct JmapClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: String,
    pub account_id: String,
    discovered_accounts: tokio::sync::OnceCell<HashMap<String, String>>,
}

impl JmapClient {
    pub fn new(account: &impl ServerAccount) -> Self {
        let auth_header = auth_header_for_account(account);
        let base_url = account.server_url().trim_end_matches('/').to_string();
        let account_id = account.id().to_string();
        Self {
            client: reqwest::Client::new(),
            base_url,
            auth_header,
            account_id,
            discovered_accounts: tokio::sync::OnceCell::new(),
        }
    }

    pub async fn account_id_for(&self, capability: &str) -> Result<String, String> {
        let accounts = self
            .discovered_accounts
            .get_or_try_init(|| async {
                let response = self
                    .client
                    .get(format!("{}/jmap/session", self.base_url))
                    .header("Authorization", &self.auth_header)
                    .header("Accept", "application/json")
                    .send()
                    .await
                    .map_err(|error| error.to_string())?
                    .error_for_status()
                    .map_err(|error| error.to_string())?;
                let session: Value = response.json().await.map_err(|error| error.to_string())?;
                let mut accounts = HashMap::new();
                if let Some(primary) = session.get("primaryAccounts").and_then(Value::as_object) {
                    for (capability, account_id) in primary {
                        if let Some(account_id) = account_id.as_str() {
                            accounts.insert(capability.clone(), account_id.to_owned());
                        }
                    }
                }
                if let Some(session_accounts) = session.get("accounts").and_then(Value::as_object) {
                    for (account_id, account) in session_accounts {
                        if let Some(capabilities) = account
                            .get("accountCapabilities")
                            .and_then(Value::as_object)
                        {
                            for capability in capabilities.keys() {
                                accounts
                                    .entry(capability.clone())
                                    .or_insert_with(|| account_id.clone());
                            }
                        }
                    }
                }
                Ok::<_, String>(accounts)
            })
            .await?;
        Ok(accounts
            .get(capability)
            .cloned()
            .unwrap_or_else(|| self.account_id.clone()))
    }

    pub async fn request(
        &self,
        capabilities: &[&str],
        method_calls: Vec<Value>,
    ) -> Result<Value, String> {
        let url = format!("{}/jmap", self.base_url);
        let body = json!({
            "using": capabilities,
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

        let result: Value = response.json().await.map_err(|e| e.to_string())?;
        if let Some(responses) = result.get("methodResponses").and_then(Value::as_array) {
            for response in responses {
                if response.get(0).and_then(Value::as_str) == Some("error") {
                    let error = response.get(1).cloned().unwrap_or(Value::Null);
                    let description = error
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| error.to_string());
                    return Err(description);
                }
                if let Some(body) = response.get(1).and_then(Value::as_object) {
                    for field in ["notCreated", "notUpdated", "notDestroyed"] {
                        if let Some(errors) = body
                            .get(field)
                            .and_then(Value::as_object)
                            .filter(|errors| !errors.is_empty())
                        {
                            return Err(Value::Object(errors.clone()).to_string());
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    pub fn extract_response<'a>(&self, data: &'a Value, method_name: &str) -> Option<&'a Value> {
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

    fn default_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header).unwrap(),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
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
    fn jmap_client_new_sets_fields() {
        let account = test_account();
        let client = JmapClient::new(&account);
        assert_eq!(client.account_id, "acc-1");
        assert_eq!(client.base_url, "https://example.com");
        assert!(client.auth_header.starts_with("Basic "));
    }

    #[test]
    fn extract_response_finds_method_by_name() {
        let client = JmapClient::new(&test_account());
        let data = serde_json::json!({
            "methodResponses": [
                ["Mailbox/get", { "list": [] }, "0"],
                ["Email/get", { "notFound": [] }, "1"]
            ]
        });
        let args = client.extract_response(&data, "Email/get").unwrap();
        assert!(args.get("notFound").is_some());
        assert!(client.extract_response(&data, "Thread/get").is_none());
    }

    #[test]
    fn extract_response_missing_or_invalid() {
        let client = JmapClient::new(&test_account());
        assert!(
            client
                .extract_response(&serde_json::json!({}), "x")
                .is_none()
        );
        assert!(
            client
                .extract_response(&serde_json::json!({ "methodResponses": "bad" }), "x")
                .is_none()
        );
    }
}
