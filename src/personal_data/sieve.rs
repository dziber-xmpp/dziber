use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufStream, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::models::account::{MailAccount, MailSecurity, ServerAccount};

enum SieveConnection {
    Plain(Box<TcpStream>),
    Tls(Box<TlsStream<TcpStream>>),
}

impl fmt::Debug for SieveConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SieveConnection").finish()
    }
}

impl Unpin for SieveConnection {}

impl AsyncRead for SieveConnection {
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

impl AsyncWrite for SieveConnection {
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

pub struct ManageSieveClient {
    stream: BufStream<SieveConnection>,
}

impl ManageSieveClient {
    pub async fn connect(account: &MailAccount) -> Result<Self, String> {
        let config = account
            .sieve_config
            .as_ref()
            .ok_or("No ManageSieve configuration for this account")?;

        let tcp = TcpStream::connect((config.server.as_str(), config.port))
            .await
            .map_err(|e| format!("ManageSieve connect failed: {}", e))?;

        let mut client = match config.security {
            MailSecurity::Tls => {
                let tls = tls_connector();
                let domain = server_name(&config.server)?;
                let stream = tls
                    .connect(domain, tcp)
                    .await
                    .map_err(|e| format!("ManageSieve TLS failed: {}", e))?;
                ManageSieveClient {
                    stream: BufStream::new(SieveConnection::Tls(Box::new(stream))),
                }
            }
            MailSecurity::StartTls => {
                let mut plain = ManageSieveClient {
                    stream: BufStream::new(SieveConnection::Plain(Box::new(tcp))),
                };
                let _ = plain.read_response().await?;
                plain.send_line("STARTTLS").await?;
                let lines = plain.read_response().await?;
                let status = lines.last().map(|s| s.as_str()).unwrap_or("");
                if !status.starts_with("OK") {
                    return Err(format!("STARTTLS failed: {}", status));
                }
                let tcp = match plain.stream.into_inner() {
                    SieveConnection::Plain(tcp) => *tcp,
                    _ => return Err("unexpected TLS stream during STARTTLS".to_string()),
                };
                let tls = tls_connector();
                let domain = server_name(&config.server)?;
                let stream = tls
                    .connect(domain, tcp)
                    .await
                    .map_err(|e| format!("ManageSieve TLS failed: {}", e))?;
                ManageSieveClient {
                    stream: BufStream::new(SieveConnection::Tls(Box::new(stream))),
                }
            }
            MailSecurity::None => ManageSieveClient {
                stream: BufStream::new(SieveConnection::Plain(Box::new(tcp))),
            },
        };

        // Read banner and any capability lines until the first OK.
        let _ = client.read_response().await?;
        client.authenticate(account).await?;
        Ok(client)
    }

    pub async fn list_scripts(&mut self) -> Result<Vec<(String, bool)>, String> {
        self.send_line("LISTSCRIPTS").await?;
        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("LISTSCRIPTS failed: {}", status));
        }
        Ok(parse_script_list(&lines.join("\n")))
    }

    pub async fn get_script(&mut self, name: &str) -> Result<String, String> {
        self.send_line(&format!("GETSCRIPT {}", quote_sieve_string(name)))
            .await?;
        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("GETSCRIPT failed: {}", status));
        }
        extract_literal(&lines.join("\n"))
            .ok_or_else(|| "GETSCRIPT response missing script content".to_string())
    }

    pub async fn put_script(&mut self, name: &str, content: &str) -> Result<(), String> {
        let bytes = content.as_bytes();
        self.send_line(&format!(
            "PUTSCRIPT {} {{{}+}}",
            quote_sieve_string(name),
            bytes.len()
        ))
        .await?;
        self.stream
            .write_all(bytes)
            .await
            .map_err(|e| e.to_string())?;
        self.stream
            .write_all(b"\r\n")
            .await
            .map_err(|e| e.to_string())?;
        self.stream.flush().await.map_err(|e| e.to_string())?;

        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("PUTSCRIPT failed: {}", status));
        }
        Ok(())
    }

    pub async fn delete_script(&mut self, name: &str) -> Result<(), String> {
        self.send_line(&format!("DELETESCRIPT {}", quote_sieve_string(name)))
            .await?;
        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("DELETESCRIPT failed: {}", status));
        }
        Ok(())
    }

    pub async fn set_active(&mut self, name: &str) -> Result<(), String> {
        self.send_line(&format!("SETACTIVE {}", quote_sieve_string(name)))
            .await?;
        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("SETACTIVE failed: {}", status));
        }
        Ok(())
    }

    pub async fn logout(&mut self) -> Result<(), String> {
        let _ = self.send_line("LOGOUT").await;
        Ok(())
    }

    async fn authenticate(&mut self, account: &impl ServerAccount) -> Result<(), String> {
        let sasl = sasl_plain_for_account(account);
        self.send_line(&format!("AUTHENTICATE \"PLAIN\" \"{}\"", sasl))
            .await?;
        let lines = self.read_response().await?;
        let status = lines.last().map(|s| s.as_str()).unwrap_or("");
        if !status.starts_with("OK") {
            return Err(format!("ManageSieve authentication failed: {}", status));
        }
        Ok(())
    }

    async fn send_line(&mut self, line: &str) -> Result<(), String> {
        self.stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write failed: {}", e))?;
        self.stream
            .write_all(b"\r\n")
            .await
            .map_err(|e| format!("write failed: {}", e))?;
        self.stream
            .flush()
            .await
            .map_err(|e| format!("flush failed: {}", e))
    }

    /// Read lines from the server until a status line (OK/NO/BYE) is reached.
    /// Any server literals encountered in intermediate lines are expanded inline.
    async fn read_response(&mut self) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        loop {
            let mut buffer = String::new();
            let n = self
                .stream
                .read_line(&mut buffer)
                .await
                .map_err(|e| format!("read failed: {}", e))?;
            if n == 0 {
                return Err("ManageSieve server closed connection".to_string());
            }
            let line = buffer.trim_end_matches('\r').trim_end_matches('\n').to_string();
            if line.starts_with("OK") || line.starts_with("NO") || line.starts_with("BYE") {
                lines.push(line);
                return Ok(lines);
            }
            // If the line ends with a literal marker, expand it and continue reading.
            if let Some(size) = literal_size(&line) {
                let mut literal = vec![0u8; size];
                self.stream
                    .read_exact(&mut literal)
                    .await
                    .map_err(|e| format!("literal read failed: {}", e))?;
                // Consume trailing CRLF.
                let mut crlf = [0u8; 2];
                self.stream
                    .read_exact(&mut crlf)
                    .await
                    .map_err(|e| format!("literal CRLF read failed: {}", e))?;
                // Store the line with literal content appended.
                let content = String::from_utf8_lossy(&literal);
                lines.push(format!("{} {}", line, content));
            } else {
                lines.push(line);
            }
        }
    }
}

fn sasl_plain_for_account(account: &impl ServerAccount) -> String {
    use crate::personal_data::auth_for_account;
    let (username, password) = auth_for_account(account);
    let mut credentials = Vec::new();
    credentials.push(0u8);
    credentials.extend_from_slice(username.as_bytes());
    credentials.push(0u8);
    credentials.extend_from_slice(password.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(&credentials)
}

fn quote_sieve_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn literal_size(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let start = trimmed.rfind('{')?;
    let end = trimmed[start + 1..].find('}')? + start + 1;
    let inner = &trimmed[start + 1..end];
    let size_str = inner.strip_suffix('+').unwrap_or(inner);
    size_str.parse().ok()
}

fn parse_script_list(response: &str) -> Vec<(String, bool)> {
    let mut scripts = Vec::new();
    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("OK") || line.starts_with("NO") || line.starts_with("BYE") {
            continue;
        }
        let mut active = false;
        let mut name = String::new();
        for part in split_sieve_response(line) {
            let part = part.trim();
            if part.eq_ignore_ascii_case("ACTIVE") {
                active = true;
            } else if !part.is_empty() {
                name = unquote(part).unwrap_or_else(|| part.to_string());
            }
        }
        if !name.is_empty() {
            scripts.push((name, active));
        }
    }
    scripts
}

fn extract_literal(response: &str) -> Option<String> {
    for line in response.lines() {
        let line = line.trim();
        if let Some(size) = literal_size(line) {
            // The literal content is the remainder of the line after the marker.
            let marker_end = line.rfind('{').unwrap() + size.to_string().len() + 2;
            let remainder = line.get(marker_end..).unwrap_or("").trim();
            if !remainder.is_empty() {
                return Some(unquote(remainder).unwrap_or_else(|| remainder.to_string()));
            }
        }
    }
    None
}

fn split_sieve_response(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if i > start {
                    parts.push(&line[start..i]);
                }
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if start < line.len() {
        parts.push(&line[start..]);
    }
    parts
}

fn unquote(s: &str) -> Option<String> {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        Some(
            s[1..s.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\\\", "\\"),
        )
    } else {
        None
    }
}

fn server_name(server: &str) -> Result<tokio_rustls::rustls::pki_types::ServerName<'static>, String> {
    tokio_rustls::rustls::pki_types::ServerName::try_from(server.to_string())
        .map_err(|e| format!("invalid server name: {:?}", e))
}

fn tls_connector() -> tokio_rustls::TlsConnector {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_sieve_string() {
        assert_eq!(quote_sieve_string("vacation"), "\"vacation\"");
        assert_eq!(quote_sieve_string("foo\\bar"), "\"foo\\\\bar\"");
        assert_eq!(quote_sieve_string("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn test_parse_script_list() {
        let response = "\"spam\" ACTIVE\r\n\"vacation\"\r\nOK\r\n";
        let scripts = parse_script_list(response);
        assert_eq!(scripts, vec![
            ("spam".to_string(), true),
            ("vacation".to_string(), false),
        ]);
    }

    #[test]
    fn test_literal_size() {
        assert_eq!(literal_size("{12}"), Some(12));
        assert_eq!(literal_size("{0+}"), Some(0));
        assert_eq!(literal_size("foo {123}"), Some(123));
        assert_eq!(literal_size("foo"), None);
    }

    #[test]
    fn test_unquote() {
        assert_eq!(unquote("\"hello\""), Some("hello".to_string()));
        assert_eq!(unquote("\"say \\\"hi\\\"\""), Some("say \"hi\"".to_string()));
        assert_eq!(unquote("plain"), None);
    }

    #[test]
    fn sasl_plain_for_account_basic() {
        use crate::models::account::{AuthMode, MailAccount, MailProtocol};
        use base64::Engine;

        let account = MailAccount {
            id: "a".to_string(),
            server_url: "s".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
            auth_mode: AuthMode::Basic,
            mail_protocol: MailProtocol::Jmap,
            sieve_config: None,
        };
        let encoded = sasl_plain_for_account(&account);
        let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        assert_eq!(decoded, b"\0alice\0secret");
    }

    #[test]
    fn sasl_plain_for_account_stalwart() {
        use crate::models::account::{AuthMode, MailAccount, MailProtocol};
        use base64::Engine;

        let account = MailAccount {
            id: "a".to_string(),
            server_url: "s".to_string(),
            username: "alice".to_string(),
            password: "ignored".to_string(),
            auth_mode: AuthMode::StalwartImpersonation {
                admin_user: "admin".to_string(),
                admin_pass: "adminpass".to_string(),
            },
            mail_protocol: MailProtocol::Jmap,
            sieve_config: None,
        };
        let encoded = sasl_plain_for_account(&account);
        let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        assert_eq!(decoded, b"\0alice%admin\0adminpass");
    }

    #[test]
    fn split_sieve_response_respects_quotes_and_escapes() {
        assert_eq!(
            split_sieve_response(r#""foo" "bar""#),
            vec!["\"foo\"", "\"bar\""]
        );
        assert_eq!(
            split_sieve_response(r#"a "b c" d"#),
            vec!["a", "\"b c\"", "d"]
        );
        assert_eq!(
            split_sieve_response("spaced   out"),
            vec!["spaced", "out"]
        );
        assert_eq!(
            split_sieve_response(r#""escaped \"quote\"""#),
            vec![r#""escaped \"quote\"""#]
        );
    }

    #[test]
    fn extract_literal_extracts_content_after_marker() {
        assert_eq!(
            extract_literal("{11} hello world\r\nOK"),
            Some("hello world".to_string())
        );
        assert_eq!(
            extract_literal("{7} \"vacation\"\r\nOK"),
            Some("vacation".to_string())
        );
        assert_eq!(extract_literal("OK\r\n"), None);
    }
}
