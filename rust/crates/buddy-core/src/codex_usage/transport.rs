//! Fixed-host, read-only allowance request using the user's local Codex login.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::Value;

use super::{MAX_RESPONSE_BYTES, ParseError, UsageSnapshot};

const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTH_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    MissingLogin,
    ExpiredLogin,
    Unavailable,
    Http(u16),
    Parse(ParseError),
}
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLogin => {
                f.write_str("No Codex ChatGPT login found. Run codex and sign in with ChatGPT.")
            }
            Self::ExpiredLogin => {
                f.write_str("The Codex ChatGPT login expired. Run codex and sign in again.")
            }
            Self::Unavailable => f.write_str("Codex usage is unavailable. Try refreshing."),
            Self::Http(status) => write!(f, "ChatGPT usage returned HTTP {status}."),
            Self::Parse(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for FetchError {}

// Deliberately no Debug or Serialize: credentials never enter logs or UI state.
struct Credentials {
    token: String,
    account_id: Option<String>,
}
fn credentials(path: &Path) -> Result<Credentials, FetchError> {
    let file = std::fs::File::open(path).map_err(|_| FetchError::MissingLogin)?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(FetchError::MissingLogin);
    }
    let mut bytes = Vec::new();
    file.take(AUTH_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FetchError::MissingLogin)?;
    if bytes.len() as u64 > AUTH_LIMIT {
        return Err(FetchError::MissingLogin);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| FetchError::MissingLogin)?;
    let tokens = &value["tokens"];
    let string = |snake: &str, camel: &str| {
        tokens
            .get(snake)
            .filter(|value| !value.is_null())
            .or_else(|| tokens.get(camel))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    let token = string("access_token", "accessToken").ok_or(FetchError::MissingLogin)?;
    let account_id = string("account_id", "accountId");
    if token.len() > 8192
        || token.chars().any(char::is_control)
        || account_id
            .as_ref()
            .is_some_and(|id| id.len() > 256 || id.chars().any(char::is_control))
    {
        return Err(FetchError::MissingLogin);
    }
    Ok(Credentials { token, account_id })
}
fn auth_path() -> Result<PathBuf, FetchError> {
    let base = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|home| PathBuf::from(home).join(".codex"))
        })
        .filter(|path| path.is_absolute())
        .ok_or(FetchError::MissingLogin)?;
    Ok(base.join("auth.json"))
}
/// This local login belongs to the first configured Codex usage account only.
/// The caller enforces that assignment before invoking this function.
pub async fn fetch() -> Result<UsageSnapshot, FetchError> {
    if std::env::var_os("BUDDY_COPILOT_FIXTURE").is_some() {
        let owned = crate::config::config_path()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("native-assistant")))
            .ok_or(FetchError::Unavailable)?;
        crate::copilot_fixture::load(&owned)
            .map_err(|_| FetchError::Unavailable)?
            .ok_or(FetchError::Unavailable)?;
        if std::env::var("BUDDY_GITHUB_FIXTURE").as_deref() != Ok("1") {
            return Err(FetchError::Unavailable);
        }
        match std::env::var("BUDDY_CODEX_FIXTURE_SCENARIO").as_deref() {
            Ok("missing-auth") => return Err(FetchError::MissingLogin),
            Ok("normal") => {}
            _ => return Err(FetchError::Unavailable),
        }
        let account = crate::github::AccountScope::new("synthetic-account", "fixture-org")
            .map_err(|_| FetchError::Unavailable)?;
        let value = crate::github::GitHubClients::configured()
            .for_account(account)
            .request(
                crate::github::ApiRequest::get("/copilot-fixture/codex-usage"),
                true,
            )
            .await
            .map_err(|_| FetchError::Unavailable)?
            .value
            .body;
        return super::parse(
            &serde_json::to_vec(&value).map_err(|_| FetchError::Unavailable)?,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(FetchError::Parse);
    }
    let path = auth_path()?;
    let credentials = tokio::task::spawn_blocking(move || credentials(&path))
        .await
        .map_err(|_| FetchError::MissingLogin)??;
    request(ENDPOINT, credentials, chrono::Utc::now().timestamp_millis()).await
}
async fn request(
    endpoint: &str,
    credentials: Credentials,
    now: i64,
) -> Result<UsageSnapshot, FetchError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("hs-buddy")
        .build()
        .map_err(|_| FetchError::Unavailable)?;
    let mut headers = HeaderMap::new();
    let mut auth = HeaderValue::from_str(&format!("Bearer {}", credentials.token))
        .map_err(|_| FetchError::MissingLogin)?;
    auth.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth);
    if let Some(id) = credentials.account_id {
        let mut value = HeaderValue::from_str(&id).map_err(|_| FetchError::MissingLogin)?;
        value.set_sensitive(true);
        headers.insert("ChatGPT-Account-Id", value);
    }
    let mut response = client
        .get(endpoint)
        .headers(headers)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| FetchError::Unavailable)?;
    let status = response.status();
    if matches!(status.as_u16(), 401 | 403) {
        return Err(FetchError::ExpiredLogin);
    }
    if !status.is_success() {
        return Err(FetchError::Http(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(FetchError::Parse(ParseError::TooLarge));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| FetchError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(FetchError::Parse(ParseError::TooLarge));
        }
        bytes.extend_from_slice(&chunk);
    }
    super::parse(&bytes, now).map_err(FetchError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> Credentials {
        Credentials {
            token: "synthetic-token".into(),
            account_id: Some("synthetic-account".into()),
        }
    }
    fn server(
        status: &str,
        headers: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        );
        let thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => panic!("owned usage fixture received no request before its deadline"),
                }
            };
            // Windows inherits the listener's mode on accepted sockets.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            // The client can reject Content-Length before reading the body.
            // A synchronous join must not wait forever for that body to drain
            // while the current-thread runtime cannot poll connection cleanup.
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 16384);
            }
            stream.write_all(response.as_bytes()).ok();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });
        (format!("http://{address}/usage"), thread)
    }
    #[test]
    fn reads_only_bounded_supported_login_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.json");
        assert!(matches!(credentials(&path), Err(FetchError::MissingLogin)));
        std::fs::write(
            &path,
            r#"{"tokens":{"accessToken":"synthetic-token","accountId":"synthetic-account"}}"#,
        )
        .unwrap();
        let parsed = credentials(&path).unwrap();
        assert_eq!(parsed.token, "synthetic-token");
        assert_eq!(parsed.account_id.as_deref(), Some("synthetic-account"));
        std::fs::write(&path, vec![b' '; AUTH_LIMIT as usize + 1]).unwrap();
        assert!(matches!(credentials(&path), Err(FetchError::MissingLogin)));
        std::fs::write(&path, r#"{"tokens":{"access_token":"unsafe\nheader"}}"#).unwrap();
        assert!(matches!(credentials(&path), Err(FetchError::MissingLogin)));
    }
    #[tokio::test]
    async fn actual_http_preserves_login_identity_and_parses_usage() {
        let (endpoint, server) = server(
            "200 OK",
            "",
            r#"{"rate_limit":{"primary_window":{"used_percent":25,"reset_at":18000,"limit_window_seconds":18000}}}"#,
        );
        let snapshot = request(&endpoint, fixture(), 7_200_000).await.unwrap();
        assert_eq!(snapshot.windows[0].projected_percent, 62.5);
        let request = server.join().unwrap();
        assert!(request.contains("authorization: bearer synthetic-token\r\n"));
        assert!(request.contains("chatgpt-account-id: synthetic-account\r\n"));
    }
    #[tokio::test]
    async fn rejects_redirects_expired_logins_and_oversized_responses_without_exposing_bodies() {
        for (status, headers, body, expected) in [
            (
                "302 Found",
                "Location: http://127.0.0.1:1/forbidden\r\n",
                "private-body",
                FetchError::Http(302),
            ),
            (
                "401 Unauthorized",
                "",
                "private-body",
                FetchError::ExpiredLogin,
            ),
            (
                "403 Forbidden",
                "",
                "private-body",
                FetchError::ExpiredLogin,
            ),
            ("500 Error", "", "private-body", FetchError::Http(500)),
        ] {
            let (endpoint, server) = server(status, headers, body);
            assert_eq!(
                request(&endpoint, fixture(), 0).await.unwrap_err(),
                expected
            );
            server.join().unwrap();
        }
        let body = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let (endpoint, server) = server("200 OK", "", &body);
        assert_eq!(
            request(&endpoint, fixture(), 0).await.unwrap_err(),
            FetchError::Parse(ParseError::TooLarge)
        );
        server.join().unwrap();
    }
}
