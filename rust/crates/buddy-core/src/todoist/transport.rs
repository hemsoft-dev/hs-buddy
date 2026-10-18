//! A connection owns one credential and write scope. Explicit retries keep both.
use std::time::Duration;

use reqwest::{StatusCode, Url, header};

use super::{
    Error, Id, MAX_RESPONSE_BYTES, Page, Project, ReadPages, Task, WriteOutcome, WriteRequest,
    WriteScope, bounded_text, parse_projects, parse_tasks, project_pages, task_pages,
};

#[path = "read_consistency.rs"]
mod read_consistency;

const API: &str = "https://api.todoist.com/api/v1/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    InvalidCredential,
    Unavailable,
    Authentication,
    RateLimited,
    Service,
    Redirect,
    FixtureUnavailable,
    Domain(Error),
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FixtureUnavailable => "The disposable Todoist fixture is unavailable. No service request was sent.",
            Self::InvalidCredential => "Enter a valid Todoist API token.",
            Self::Unavailable => "Could not reach Todoist. Existing tasks are retained. Check your connection and try again.",
            Self::Authentication => "Todoist rejected this connection. Check its API token before retrying.",
            Self::RateLimited => "Todoist is limiting requests. Wait before trying again.",
            Self::Service => "Todoist could not finish this request. Existing tasks are retained.",
            Self::Redirect => "Todoist returned an unexpected redirect. This connection did not follow it.",
            Self::Domain(error) => return error.fmt(formatter),
        })
    }
}
impl std::error::Error for TransportError {}
impl From<Error> for TransportError {
    fn from(value: Error) -> Self {
        Self::Domain(value)
    }
}

/// No Debug: filters can contain private project names and task text.
pub enum ReadScope {
    Active,
    Filter(String),
    Project(Id),
}

/// A transport error does not prove whether a write was applied. The caller
/// retains the original request and offers only an explicit same-UUID retry.
pub struct WriteReply {
    pub outcome: WriteOutcome,
    pub problem: Option<TransportError>,
}

/// No Debug and no credential replacement. Reconnecting creates a new scope.
pub struct Client {
    http: reqwest::Client,
    base: Url,
    authorization: header::HeaderValue,
    scope: WriteScope,
    read_marker: tokio::sync::Mutex<Option<String>>,
}
impl Client {
    /// CI desktop qualification only. Real tokens cannot enter this transport.
    pub fn for_desktop_fixture(
        token: &str,
        config: &std::path::Path,
    ) -> Result<(Self, chrono::DateTime<chrono::Utc>), TransportError> {
        let connection = super::fixture::load(token, config)?;
        let client = Self::build(token, connection.api, false)?;
        Ok((client, connection.now))
    }
    /// Persist these bytes before dispatch. The caller owns atomic profile-local
    /// storage and retains unconfirmed changes until explicitly resolved.
    pub async fn checkpoint(
        &self,
        attempt: &super::WriteAttempt,
        path: &std::path::Path,
    ) -> Result<Vec<u8>, TransportError> {
        self.checkpoint_with_account(attempt, path, None).await
    }

    /// Bind a planner's already loaded tasks to that same authenticated account
    /// during the actual checkpoint read, before publishing any recovery bytes.
    pub async fn checkpoint_for_account(
        &self,
        attempt: &super::WriteAttempt,
        path: &std::path::Path,
        account: &Id,
    ) -> Result<Vec<u8>, TransportError> {
        self.checkpoint_with_account(attempt, path, Some(account))
            .await
    }

    async fn checkpoint_with_account(
        &self,
        attempt: &super::WriteAttempt,
        path: &std::path::Path,
        account: Option<&Id>,
    ) -> Result<Vec<u8>, TransportError> {
        attempt.request_scope(self.scope)?;
        let profile = self.profile().await?;
        if account.is_some_and(|account| account != &profile.id) {
            return Err(Error::WrongConnection.into());
        }
        attempt.checkpoint(path, &profile.id).map_err(Into::into)
    }

    /// Re-read the authenticated account before rebinding a saved command to
    /// this connection. Recovery never dispatches the change automatically.
    pub async fn recover(
        &self,
        bytes: &[u8],
        path: &std::path::Path,
    ) -> Result<super::WriteAttempt, TransportError> {
        self.recover_with_account(bytes, path, None).await
    }

    pub async fn recover_for_account(
        &self,
        bytes: &[u8],
        path: &std::path::Path,
        account: &Id,
    ) -> Result<super::WriteAttempt, TransportError> {
        self.recover_with_account(bytes, path, Some(account)).await
    }

    async fn recover_with_account(
        &self,
        bytes: &[u8],
        path: &std::path::Path,
        account: Option<&Id>,
    ) -> Result<super::WriteAttempt, TransportError> {
        let profile = self.profile().await?;
        if account.is_some_and(|account| account != &profile.id) {
            return Err(Error::WrongConnection.into());
        }
        super::WriteAttempt::restore(bytes, path, &profile.id, self.scope).map_err(Into::into)
    }
    pub fn new(token: &str) -> Result<Self, TransportError> {
        Self::build(
            token,
            Url::parse(API).expect("static Todoist HTTPS origin"),
            true,
        )
    }
    fn build(token: &str, base: Url, https: bool) -> Result<Self, TransportError> {
        if !super::valid_token(token) {
            return Err(TransportError::InvalidCredential);
        }
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| TransportError::InvalidCredential)?;
        authorization.set_sensitive(true);
        let builder = reqwest::Client::builder()
            .user_agent("hs-buddy/1.0")
            .https_only(https)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(REQUEST_TIMEOUT);
        let builder = if https { builder } else { builder.no_proxy() };
        let http = builder.build().map_err(|_| TransportError::Unavailable)?;
        Ok(Self {
            http,
            base,
            authorization,
            scope: WriteScope::new(),
            read_marker: tokio::sync::Mutex::new(None),
        })
    }
    pub fn write_scope(&self) -> WriteScope {
        self.scope
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(
                method,
                self.base.join(path).expect("static Todoist endpoint"),
            )
            .header(header::AUTHORIZATION, self.authorization.clone())
    }
    pub async fn tasks(&self, scope: &ReadScope) -> Result<Vec<Task>, TransportError> {
        let (path, query) = match scope {
            ReadScope::Active => ("tasks", Vec::new()),
            ReadScope::Project(id) => ("tasks", vec![("project_id", id.as_str())]),
            ReadScope::Filter(filter) => {
                if filter.trim().is_empty() || !bounded_text(filter, 1024, false) {
                    return Err(Error::InvalidDraft.into());
                }
                ("tasks/filter", vec![("query", filter.as_str())])
            }
        };
        self.read(path, &query, task_pages(), parse_tasks).await
    }
    /// Never falls back to a cached account after an authentication failure.
    pub async fn profile(&self) -> Result<super::Profile, TransportError> {
        let response = self
            .request(reqwest::Method::GET, "user")
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        super::parse_profile(&bounded_body(response).await?).map_err(Into::into)
    }
    pub async fn projects(&self) -> Result<Vec<Project>, TransportError> {
        self.read("projects", &[], project_pages(), parse_projects)
            .await
    }
    async fn read<Item>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        mut pages: ReadPages<Item>,
        parse: fn(&[u8]) -> Result<Page<Item>, Error>,
    ) -> Result<Vec<Item>, TransportError> {
        tokio::time::timeout(READ_TIMEOUT, async {
            let mut marker = None;
            loop {
                let cursor = pages.request_cursor()?;
                let mut request = self
                    .request(reqwest::Method::GET, path)
                    .query(query)
                    .query(&[("limit", "100")]);
                if let Some(cursor) = &cursor {
                    request = request.query(&[("cursor", cursor)]);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|_| TransportError::Unavailable)?;
                let bytes = bounded_body(response).await?;
                let page = parse(&bytes)?;
                if pages.received_pages() == 0 && page.next_cursor.is_some() && marker.is_none() {
                    let mut held = self.read_marker.lock().await;
                    self.start_read_marker(&mut held).await?;
                    marker = Some(held);
                    // The first response preceded the watermark. Discard it
                    // and restart the entire collection under that marker.
                    continue;
                }
                pages.push(cursor.as_deref(), page)?;
                if pages.is_complete() {
                    if let Some(held) = marker.as_mut() {
                        self.finish_read_marker(held).await?;
                    }
                    return pages.finish().map_err(Into::into);
                }
            }
        })
        .await
        .map_err(|_| TransportError::Unavailable)?
    }
    /// Never retries implicitly, follows redirects, logs bodies, or returns raw
    /// reqwest/service diagnostics. Scope is checked before constructing HTTP.
    pub async fn send(&self, request: &WriteRequest) -> Result<WriteReply, TransportError> {
        request.check_scope(self.scope)?;
        let response = self
            .request(reqwest::Method::POST, "sync")
            .form(&[("commands", request.commands())])
            .send()
            .await;
        let bytes = match response {
            Ok(response) => bounded_body(response).await,
            Err(_) => Err(TransportError::Unavailable),
        };
        Ok(match bytes {
            Ok(bytes) => {
                let outcome = request.decode(&bytes);
                let problem = matches!(outcome, WriteOutcome::Unconfirmed)
                    .then_some(TransportError::Domain(Error::InvalidResponse));
                WriteReply { outcome, problem }
            }
            Err(problem) => WriteReply {
                outcome: WriteOutcome::Unconfirmed,
                problem: Some(problem),
            },
        })
    }
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, TransportError> {
    let status = response.status();
    if status.is_redirection() {
        return Err(TransportError::Redirect);
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(TransportError::Authentication);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(TransportError::RateLimited);
    }
    if !status.is_success() {
        return Err(TransportError::Service);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::ResponseTooLarge.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| TransportError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(Error::ResponseTooLarge.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
