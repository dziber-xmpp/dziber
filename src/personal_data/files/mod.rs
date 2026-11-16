use crate::{
    models::account::FilesAccount,
    personal_data::{
        auth_header_for_account,
        dav::{DavClient, DavResponse, encode_account},
    },
};

/// Stalwart WebDAV client for a user's file collection.
pub struct FileServerClient {
    account: FilesAccount,
    dav: DavClient,
    http: reqwest::Client,
    auth_header: String,
}

impl FileServerClient {
    pub fn new(account: &FilesAccount) -> Self {
        let auth_header = auth_header_for_account(account);
        Self {
            account: account.clone(),
            dav: DavClient::new(account.server_url.clone(), auth_header.clone()),
            http: reqwest::Client::new(),
            auth_header,
        }
    }

    fn resource_path(&self, path: &str) -> Result<String, String> {
        let path = path.trim_matches('/');
        if path
            .split('/')
            .any(|part| part == "." || part == ".." || part.contains(['\\', '\0']))
        {
            return Err("invalid file path".to_owned());
        }
        let path = path
            .split('/')
            .map(|part| {
                percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("/");
        Ok(format!(
            "/dav/file/{}/{path}",
            encode_account(&self.account.username)
        ))
    }

    fn absolute_url(&self, path: &str) -> String {
        format!("{}{}", self.account.server_url.trim_end_matches('/'), path)
    }

    pub async fn list(&self, path: &str) -> Result<Vec<DavResponse>, String> {
        let resource = self.resource_path(path)?;
        let xml = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:displayname/><D:getcontentlength/><D:getcontenttype/><D:getlastmodified/></D:prop></D:propfind>"#;
        let mut responses = self
            .dav
            .propfind(&resource, xml, "1")
            .await
            .map_err(|error| error.to_string())?;
        let current = resource.trim_end_matches('/');
        responses.retain(|response| !response.href.trim_end_matches('/').ends_with(current));
        Ok(responses)
    }

    pub async fn download(&self, path: &str) -> Result<(u16, String, Vec<u8>), String> {
        let resource = self.resource_path(path)?;
        let response = self
            .http
            .get(self.absolute_url(&resource))
            .header(reqwest::header::AUTHORIZATION, &self.auth_header)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| error.to_string())?
            .to_vec();
        Ok((status, content_type, bytes))
    }

    pub async fn upload(
        &self,
        path: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<u16, String> {
        let resource = self.resource_path(path)?;
        let response = self
            .http
            .put(self.absolute_url(&resource))
            .header(reqwest::header::AUTHORIZATION, &self.auth_header)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        Ok(response.status().as_u16())
    }

    pub async fn create_folder(&self, path: &str, name: &str) -> Result<u16, String> {
        validate_folder_name(name)?;
        let folder = join_path(path, name);
        let resource = format!("{}/", self.resource_path(&folder)?.trim_end_matches('/'));
        let response = self
            .http
            .request(
                reqwest::Method::from_bytes(b"MKCOL").expect("valid HTTP method"),
                self.absolute_url(&resource),
            )
            .header(reqwest::header::AUTHORIZATION, &self.auth_header)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        Ok(response.status().as_u16())
    }

    pub async fn rename_folder(&self, path: &str, name: &str) -> Result<u16, String> {
        validate_folder_name(name)?;
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return Err("cannot rename the file storage root".to_owned());
        }
        let (parent, _) = path.rsplit_once('/').unwrap_or(("", path));
        let destination = join_path(parent, name);
        let source = format!("{}/", self.resource_path(path)?.trim_end_matches('/'));
        let destination = format!(
            "{}/",
            self.absolute_url(&self.resource_path(&destination)?)
                .trim_end_matches('/')
        );
        let response = self
            .http
            .request(
                reqwest::Method::from_bytes(b"MOVE").expect("valid HTTP method"),
                self.absolute_url(&source),
            )
            .header(reqwest::header::AUTHORIZATION, &self.auth_header)
            .header("Destination", destination)
            .header("Overwrite", "F")
            .send()
            .await
            .map_err(|error| error.to_string())?;
        Ok(response.status().as_u16())
    }

    pub async fn delete_folder(&self, path: &str) -> Result<u16, String> {
        if path.trim_matches('/').is_empty() {
            return Err("cannot delete the file storage root".to_owned());
        }
        let resource = format!("{}/", self.resource_path(path)?.trim_end_matches('/'));
        let response = self
            .http
            .delete(self.absolute_url(&resource))
            .header(reqwest::header::AUTHORIZATION, &self.auth_header)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        Ok(response.status().as_u16())
    }
}

fn validate_folder_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err("enter a valid folder name".to_owned());
    }
    Ok(())
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.trim_matches('/').is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", parent.trim_matches('/'))
    }
}

type Result<T, E = String> = std::result::Result<T, E>;
