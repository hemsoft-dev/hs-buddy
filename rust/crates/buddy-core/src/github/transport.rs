use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{Value, json};

use super::{AccountScope, GitHubError};

pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 512 * 1024;

pub type ApiFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ApiResponse, GitHubError>> + Send + 'a>>;
pub type CredentialFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, GitHubError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub method: reqwest::Method,
    pub path: String,
    pub body: Option<Value>,
    /// GraphQL reads use POST but must not be treated as uncertain writes.
    pub is_write: bool,
}

impl ApiRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: reqwest::Method::GET,
            path: path.into(),
            body: None,
            is_write: false,
        }
    }

    pub fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: reqwest::Method::POST,
            path: path.into(),
            body: Some(body),
            is_write: true,
        }
    }

    pub fn graphql(query: &str, variables: Value, is_write: bool) -> Self {
        Self {
            method: reqwest::Method::POST,
            path: "/graphql".into(),
            body: Some(json!({"query":query,"variables":variables})),
            is_write,
        }
    }

    pub fn cache_key(&self) -> String {
        format!(
            "{} {} {}",
            self.method,
            self.path,
            self.body.as_ref().unwrap_or(&Value::Null)
        )
    }

    fn validate(&self) -> Result<(), GitHubError> {
        if !self.path.starts_with('/')
            || self.path.starts_with("//")
            || self.path.contains('\\')
            || self.path.chars().any(char::is_control)
            || self
                .body
                .as_ref()
                .is_some_and(|body| body.to_string().len() > MAX_REQUEST_BYTES)
        {
            return Err(GitHubError::InvalidResource);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    /// Only allowlisted rate-limit and pagination headers are retained.
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl ApiResponse {
    pub fn checked(self, is_write: bool) -> Result<Value, GitHubError> {
        let limited = self.status == 429
            || (self.status == 403
                && (self
                    .headers
                    .get("x-ratelimit-remaining")
                    .is_some_and(|v| v == "0")
                    || self.headers.contains_key("retry-after")
                    || self.body["code"] == "secondary-rate-limit"));
        if limited {
            return Err(GitHubError::RateLimited {
                retry_after_seconds: self.retry_after(),
            });
        }
        match self.status {
            200..=299 => {}
            300..=399 => return Err(GitHubError::Moved),
            401 => return Err(GitHubError::Authentication),
            403 => return Err(GitHubError::Denied),
            404 => return Err(GitHubError::NotFound),
            409 if !is_write && self.body["code"] == "empty-repository" => {
                return Err(GitHubError::EmptyRepository);
            }
            409 | 422 => return Err(GitHubError::Rejected),
            500..=599 if is_write => return Err(GitHubError::UnknownWriteResult),
            _ => return Err(GitHubError::Network),
        }
        if let Some(errors) = self.body.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            // A partial mutation response may already have committed a write.
            if is_write && self.body.get("data").is_some_and(|v| !v.is_null()) {
                return Err(GitHubError::UnknownWriteResult);
            }
            if errors.iter().any(|e| e["type"] == "RATE_LIMITED") {
                return Err(GitHubError::RateLimited {
                    retry_after_seconds: self.retry_after(),
                });
            }
            return Err(GitHubError::Denied);
        }
        Ok(self.body)
    }

    fn retry_after(&self) -> u64 {
        self.headers
            .get("retry-after")
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| {
                self.headers
                    .get("x-ratelimit-reset")
                    .and_then(|value| value.parse::<i64>().ok())
                    .map(|reset| reset.saturating_sub(chrono::Utc::now().timestamp()).max(1) as u64)
            })
            .unwrap_or(60)
            .clamp(1, 86_400)
    }
}

pub trait Transport: Send + Sync {
    fn request<'a>(&'a self, account: &'a AccountScope, request: ApiRequest) -> ApiFuture<'a>;
}

pub trait Credentials: Send + Sync {
    fn token<'a>(&'a self, account: &'a AccountScope) -> CredentialFuture<'a>;
}

pub struct GhCredentials;

impl Credentials for GhCredentials {
    fn token<'a>(&'a self, account: &'a AccountScope) -> CredentialFuture<'a> {
        Box::pin(async move {
            crate::gh::auth_token(account.username())
                .await
                .map_err(|error| match error {
                    crate::gh::GhError::NotInstalled => GitHubError::CliUnavailable,
                    crate::gh::GhError::Timeout => GitHubError::Timeout,
                    _ => GitHubError::Authentication,
                })
        })
    }
}

/// Production has one fixed API origin. It never reads an environment token or
/// sends account credentials through redirects. No request is replayed here.
pub struct HttpTransport<C = GhCredentials> {
    client: reqwest::Client,
    base: reqwest::Url,
    credentials: C,
}

fn github_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(crate::http::USER_AGENT)
        .timeout(crate::http::REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .expect("static GitHub HTTP client")
}

impl Default for HttpTransport<GhCredentials> {
    fn default() -> Self {
        Self {
            client: github_http_client(),
            base: reqwest::Url::parse("https://api.github.com").expect("static GitHub origin"),
            credentials: GhCredentials,
        }
    }
}

pub(crate) fn fixture_transport(
    base: reqwest::Url,
) -> Result<std::sync::Arc<dyn Transport>, GitHubError> {
    struct FixtureCredentials;
    impl Credentials for FixtureCredentials {
        fn token<'a>(&'a self, account: &'a AccountScope) -> CredentialFuture<'a> {
            Box::pin(async move {
                if account.organization() != "fixture-org"
                    || !matches!(
                        account.username(),
                        "fixture-one" | "fixture-two" | "synthetic-account"
                    )
                {
                    return Err(GitHubError::Authentication);
                }
                Ok(format!("native-fixture-{}", account.username()))
            })
        }
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .user_agent(crate::http::USER_AGENT)
        .timeout(crate::http::REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| GitHubError::InvalidFixture)?;
    Ok(std::sync::Arc::new(HttpTransport {
        client,
        base,
        credentials: FixtureCredentials,
    }))
}

impl<C: Credentials> Transport for HttpTransport<C> {
    fn request<'a>(&'a self, account: &'a AccountScope, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            request.validate()?;
            let url = self
                .base
                .join(&request.path)
                .map_err(|_| GitHubError::InvalidResource)?;
            if url.origin() != self.base.origin() {
                return Err(GitHubError::InvalidResource);
            }
            let token = self.credentials.token(account).await?;
            let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| GitHubError::Authentication)?;
            authorization.set_sensitive(true);
            let mut builder = self
                .client
                .request(request.method, url)
                .header(AUTHORIZATION, authorization)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2026-03-10");
            if let Some(body) = request.body {
                builder = builder.json(&body);
            }
            let failure = |error: reqwest::Error| {
                if request.is_write && !error.is_connect() {
                    GitHubError::UnknownWriteResult
                } else if error.is_timeout() {
                    GitHubError::Timeout
                } else {
                    GitHubError::Network
                }
            };
            let mut response = builder.send().await.map_err(failure)?;
            let status = response.status().as_u16();
            let headers = [
                "retry-after",
                "x-ratelimit-remaining",
                "x-ratelimit-reset",
                "link",
            ]
            .into_iter()
            .filter_map(|key| {
                response
                    .headers()
                    .get(key)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| (key.to_string(), v.to_string()))
            })
            .collect();
            // Retain only known static error codes. Optional diagnostic-body
            // failures must not replace the observed HTTP failure.
            if !(200..=299).contains(&status) {
                let mut bytes = Vec::new();
                if matches!(status, 403 | 409) {
                    while let Ok(Some(chunk)) = response.chunk().await {
                        if bytes.len().saturating_add(chunk.len()) > 4096 {
                            break;
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                }
                let body = normalized_error(status, &bytes);
                return Ok(ApiResponse {
                    status,
                    headers,
                    body,
                });
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
            {
                return Err(if request.is_write {
                    GitHubError::UnknownWriteResult
                } else {
                    GitHubError::TooLarge
                });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(failure)? {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(if request.is_write {
                        GitHubError::UnknownWriteResult
                    } else {
                        GitHubError::TooLarge
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).map_err(|_| {
                    if request.is_write {
                        GitHubError::UnknownWriteResult
                    } else {
                        GitHubError::InvalidData
                    }
                })?
            };
            Ok(ApiResponse {
                status,
                headers,
                body,
            })
        })
    }
}

fn normalized_error(status: u16, bytes: &[u8]) -> Value {
    let message = serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_ascii_lowercase));
    match (status, message.as_deref()) {
        (403, Some(message)) if message.contains("secondary rate limit") => {
            json!({"code":"secondary-rate-limit"})
        }
        (409, Some(message)) if message.contains("git repository is empty") => {
            json!({"code":"empty-repository"})
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    struct FixtureCredentials;

    impl Credentials for FixtureCredentials {
        fn token<'a>(&'a self, account: &'a AccountScope) -> CredentialFuture<'a> {
            Box::pin(async move { Ok(format!("fixture-{}", account.username())) })
        }
    }

    fn server(
        response: Vec<u8>,
        connections: usize,
    ) -> (reqwest::Url, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..connections {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 2048];
                loop {
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0, "request ended before its headers");
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                stream.write_all(&response).unwrap();
            }
            requests
        });
        (url, worker)
    }

    fn fixture(base: reqwest::Url) -> HttpTransport<FixtureCredentials> {
        HttpTransport {
            client: github_http_client(),
            base,
            credentials: FixtureCredentials,
        }
    }

    #[tokio::test]
    async fn installed_fixture_transport_scopes_fake_tokens_and_rejects_other_identities() {
        let (base, worker) = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}"
                .to_vec(),
            3,
        );
        let transport = fixture_transport(base).unwrap();
        let usernames = ["fixture-one", "fixture-two", "synthetic-account"];
        for username in usernames {
            let scope = AccountScope::new(username, "fixture-org").unwrap();
            transport
                .request(&scope, ApiRequest::get("/user"))
                .await
                .unwrap()
                .checked(false)
                .unwrap();
        }
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 3);
        for (request, username) in requests.iter().zip(usernames) {
            assert!(request.starts_with("GET /user HTTP/1.1"));
            assert!(request.contains(&format!(
                "authorization: Bearer native-fixture-{username}\r\n"
            )));
            assert_eq!(request.matches("authorization:").count(), 1);
        }
        for (username, organization) in [
            ("HemSoft", "fixture-org"),
            ("synthetic-other", "fixture-org"),
            ("synthetic-account", "other-org"),
        ] {
            let scope = AccountScope::new(username, organization).unwrap();
            assert!(matches!(
                transport.request(&scope, ApiRequest::get("/user")).await,
                Err(GitHubError::Authentication)
            ));
        }
    }

    #[tokio::test]
    async fn actual_http_requests_carry_only_the_selected_accounts_token() {
        let (base, worker) = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}"
                .to_vec(),
            2,
        );
        let transport = fixture(base);
        for user in ["first", "second"] {
            let account = AccountScope::new(user, "team").unwrap();
            let response = transport
                .request(&account, ApiRequest::get("/repos/team/repo"))
                .await
                .unwrap();
            assert_eq!(response.checked(false).unwrap(), json!({"ok":true}));
        }
        let requests = worker.join().unwrap();
        assert!(requests[0].contains("authorization: Bearer fixture-first\r\n"));
        assert!(requests[1].contains("authorization: Bearer fixture-second\r\n"));
        assert!(!requests[1].contains("fixture-first"));
        assert!(
            requests
                .iter()
                .all(|r| r.starts_with("GET /repos/team/repo HTTP/1.1"))
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_large_reads_are_rejected_before_allocation() {
        let account = AccountScope::new("first", "team").unwrap();
        let (base,worker)=server(b"HTTP/1.1 302 Found\r\nLocation: /another-resource\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),1);
        let response = fixture(base)
            .request(&account, ApiRequest::get("/repos/team/repo"))
            .await
            .unwrap();
        assert_eq!(response.checked(false), Err(GitHubError::Moved));
        assert_eq!(worker.join().unwrap().len(), 1);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        );
        let (base, worker) = server(header.into_bytes(), 1);
        assert!(matches!(
            fixture(base)
                .request(&account, ApiRequest::get("/repos/team/repo"))
                .await,
            Err(GitHubError::TooLarge)
        ));
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn a_lost_response_after_a_real_write_is_unknown_and_not_replayed() {
        let account = AccountScope::new("first", "team").unwrap();
        let (base, worker) = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_vec(),
            1,
        );
        let result = fixture(base)
            .request(
                &account,
                ApiRequest::post(
                    "/repos/team/repo/issues/7/comments",
                    json!({"body":"A confirmed fixture comment"}),
                ),
            )
            .await;
        assert!(matches!(result, Err(GitHubError::UnknownWriteResult)));
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("A confirmed fixture comment"));
    }

    fn response(status: u16, body: Value) -> ApiResponse {
        ApiResponse {
            status,
            headers: BTreeMap::new(),
            body,
        }
    }

    #[test]
    fn failures_do_not_echo_provider_text_and_never_invite_unknown_write_replay() {
        let secret = json!({"message":"credential-marker-must-not-appear"});
        assert_eq!(
            response(401, secret.clone()).checked(false),
            Err(GitHubError::Authentication)
        );
        assert_eq!(
            response(403, secret.clone()).checked(true),
            Err(GitHubError::Denied)
        );
        assert_eq!(
            response(404, secret.clone()).checked(false),
            Err(GitHubError::NotFound)
        );
        let error = response(503, secret).checked(true).unwrap_err();
        assert_eq!(error, GitHubError::UnknownWriteResult);
        assert!(!error.to_string().contains("credential-marker"));
        assert_eq!(response(200,json!({"data":{"addComment":{"id":"already-saved"}},"errors":[{"type":"FORBIDDEN"}]})).checked(true),Err(GitHubError::UnknownWriteResult));
    }

    #[test]
    fn rate_limits_use_bounded_wait_and_invalid_targets_are_rejected() {
        let mut limited = response(403, json!({}));
        limited
            .headers
            .insert("x-ratelimit-remaining".into(), "0".into());
        limited.headers.insert("retry-after".into(), "120".into());
        assert_eq!(
            limited.checked(false),
            Err(GitHubError::RateLimited {
                retry_after_seconds: 120
            })
        );
        for target in [
            "https://other.example/x",
            "//other.example/x",
            "/repos\\escape",
            "/x\nHeader: x",
        ] {
            assert_eq!(
                ApiRequest::get(target).validate(),
                Err(GitHubError::InvalidResource)
            );
        }
        assert!(
            ApiRequest::get("/repos/org/repo/pulls?page=2")
                .validate()
                .is_ok()
        );
    }
}
