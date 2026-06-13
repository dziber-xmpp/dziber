use serde_json::{Value, json};

use crate::models::account::ServerAccount;
use crate::personal_data::auth_header_for_account;

pub struct JmapClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: String,
    pub account_id: String,
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
        }
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

        response.json().await.map_err(|e| e.to_string())
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
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header).unwrap(),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }
}
