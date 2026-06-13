use percent_encoding::{AsciiSet, CONTROLS, percent_encode};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, IF_MATCH};
use roxmltree::{Document, Node};

const FRAGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'%')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'\\')
    .add(b'^')
    .add(b'~')
    .add(b'[')
    .add(b']');

pub fn encode_account(email: &str) -> String {
    percent_encode(email.as_bytes(), FRAGMENT).to_string()
}

#[derive(Debug, Clone, Default)]
pub struct DavProperty {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct DavResponse {
    pub href: String,
    pub status: u16,
    pub props: Vec<DavProperty>,
}

impl DavResponse {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.value.as_str())
    }

    pub fn resource_type(&self) -> Vec<String> {
        self.props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("resourcetype"))
            .map(|p| {
                p.value
                    .split('|')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn local_name_eq(node: Node, name: &str) -> bool {
    node.tag_name().name().eq_ignore_ascii_case(name)
}

pub fn parse_multistatus(xml: &str) -> Vec<DavResponse> {
    let doc = Document::parse(xml).unwrap_or_else(|_| Document::parse("<multistatus/>").unwrap());
    let mut responses = Vec::new();

    for response in doc.root_element().children() {
        if !local_name_eq(response, "response") {
            continue;
        }

        let mut dav_response = DavResponse::default();

        for child in response.children() {
            if local_name_eq(child, "href") {
                dav_response.href = child.text().unwrap_or("").to_string();
            } else if local_name_eq(child, "status") {
                let text = child.text().unwrap_or("");
                dav_response.status = text
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(200);
            } else if local_name_eq(child, "propstat") {
                let mut status = 200;
                let mut props = Vec::new();

                for ps_child in child.children() {
                    if local_name_eq(ps_child, "status") {
                        let text = ps_child.text().unwrap_or("");
                        status = text
                            .split_whitespace()
                            .nth(1)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(200);
                    } else if local_name_eq(ps_child, "prop") {
                        for prop in ps_child.children().filter(|n| n.is_element()) {
                            let name = prop.tag_name().name().to_string();
                            let value = if name.eq_ignore_ascii_case("resourcetype") {
                                prop.children()
                                    .filter(|n| n.is_element())
                                    .map(|n| n.tag_name().name().to_string())
                                    .collect::<Vec<_>>()
                                    .join("|")
                            } else {
                                prop.text().unwrap_or("").to_string()
                            };
                            props.push(DavProperty { name, value });
                        }
                    }
                }

                if status < 300 {
                    dav_response.props.extend(props);
                }
                if dav_response.status == 0 || status < dav_response.status {
                    dav_response.status = status;
                }
            }
        }

        responses.push(dav_response);
    }

    responses
}

pub struct DavClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: String,
}

impl DavClient {
    pub fn new(base_url: String, auth_header: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            auth_header,
        }
    }

    fn build_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            let base = self.base_url.trim_end_matches('/');
            let path = path.trim_start_matches('/');
            format!("{}/{}", base, path)
        }
    }

    fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header).unwrap(),
        );
        headers
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<String>,
        content_type: Option<&str>,
        depth: Option<&str>,
        if_match: Option<&str>,
    ) -> Result<(u16, String), reqwest::Error> {
        let mut headers = self.default_headers();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type.unwrap_or("application/xml; charset=utf-8"))
                .unwrap(),
        );
        if let Some(d) = depth {
            headers.insert(
                HeaderName::from_static("depth"),
                HeaderValue::from_str(d).unwrap(),
            );
        }
        if let Some(etag) = if_match {
            headers.insert(IF_MATCH, HeaderValue::from_str(etag).unwrap());
        }

        let mut req = self
            .client
            .request(method, self.build_url(path))
            .headers(headers);
        if let Some(b) = body {
            req = req.body(b);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        Ok((status, text))
    }

    pub async fn propfind(
        &self,
        path: &str,
        body: &str,
        depth: &str,
    ) -> Result<Vec<DavResponse>, reqwest::Error> {
        let (status, text) = self
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                path,
                Some(body.to_string()),
                Some("application/xml; charset=utf-8"),
                Some(depth),
                None,
            )
            .await?;

        if status >= 400 {
            return Ok(Vec::new());
        }

        Ok(parse_multistatus(&text))
    }

    pub async fn report(
        &self,
        path: &str,
        body: &str,
        depth: &str,
    ) -> Result<Vec<DavResponse>, reqwest::Error> {
        let (status, text) = self
            .request(
                reqwest::Method::from_bytes(b"REPORT").unwrap(),
                path,
                Some(body.to_string()),
                Some("application/xml; charset=utf-8"),
                Some(depth),
                None,
            )
            .await?;

        if status >= 400 {
            return Ok(Vec::new());
        }

        Ok(parse_multistatus(&text))
    }

    pub async fn put(
        &self,
        path: &str,
        body: String,
        content_type: &str,
        if_match: Option<&str>,
    ) -> Result<(u16, String), reqwest::Error> {
        self.request(
            reqwest::Method::PUT,
            path,
            Some(body),
            Some(content_type),
            None,
            if_match,
        )
        .await
    }

    pub async fn delete(&self, path: &str, if_match: Option<&str>) -> Result<u16, reqwest::Error> {
        let (status, _) = self
            .request(reqwest::Method::DELETE, path, None, None, None, if_match)
            .await?;
        Ok(status)
    }

    /// Try to discover the home-set path via the server's .well-known redirect.
    /// Returns the path (e.g. "/dav/card/alice/") if a redirect is followed.
    pub async fn discover_home_set(&self, kind: &str) -> Option<String> {
        let url = self.build_url(&format!("/.well-known/{}", kind));
        let resp = self
            .client
            .get(&url)
            .headers(self.default_headers())
            .send()
            .await
            .ok()?;
        let path = resp.url().path().to_string();
        // Ensure the path ends with a slash so callers can append names safely.
        if path.len() > 1 {
            Some(if path.ends_with('/') { path } else { format!("{}/", path) })
        } else {
            None
        }
    }
}

pub fn extract_rel_path(href: &str, prefix: &str) -> String {
    href.split(&format!("/dav/{}/", prefix))
        .nth(1)
        .unwrap_or("")
        .to_string()
}
