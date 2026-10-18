//! Scoped reads rebuild each request from captured filters, not next URLs.
use std::time::Duration;

use reqwest::{StatusCode, Url, header};

use super::{
    AccountId, DateRange, DaySchedule, Error, MAX_RESPONSE_BYTES, READ_PAGE_SIZE, ReadPages,
    Worklog, WorklogId, parse_schedule, parse_worklog, parse_worklog_attributes, parse_worklogs,
};

const API: &str = "https://api.tempo.io/4/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    InvalidCredential,
    Unavailable,
    Authentication,
    RateLimited,
    Service,
    NotFound,
    Redirect,
    Domain(Error),
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCredential => "Enter a valid Tempo API token.",
            Self::Unavailable => "Could not reach Tempo. Existing worklogs are retained. Check your connection and refresh.",
            Self::Authentication => "Tempo rejected this connection. Check its API token before refreshing.",
            Self::RateLimited => "Tempo is limiting requests. Wait before refreshing.",
            Self::Service => "Tempo could not load these worklogs. Existing worklogs are retained.",
            Self::NotFound => "This Tempo worklog is unavailable. The existing draft is retained. Refresh before saving.",
            Self::Redirect => "Tempo returned an unexpected redirect. This connection did not follow it.",
            Self::Domain(error) => return error.fmt(formatter),
        })
    }
}
impl std::error::Error for TransportError {}
impl From<Error> for TransportError {
    fn from(error: Error) -> Self {
        Self::Domain(error)
    }
}

/// No Debug or credential replacement. A new connection owns its own token.
pub struct Client {
    http: reqwest::Client,
    base: Url,
    authorization: header::HeaderValue,
    scope: super::WriteScope,
}
impl Client {
    pub(super) async fn send_captured(
        &self,
        request: &super::WriteRequest,
    ) -> Result<super::change::WriteReply, TransportError> {
        request.check_scope(self.scope)?;
        let kind = request.draft().kind();
        let route = match kind {
            super::DraftKind::Create => "worklogs".to_owned(),
            _ => format!(
                "worklogs/{}",
                request
                    .draft()
                    .worklog_id()
                    .ok_or(Error::InvalidDraft)?
                    .get()
            ),
        };
        let url = self.base.join(&route).map_err(|_| Error::InvalidResponse)?;
        let builder = match kind {
            super::DraftKind::Create => self.http.post(url),
            super::DraftKind::Update => self.http.put(url),
            super::DraftKind::Delete => self.http.delete(url),
        };
        let builder = builder.header(header::AUTHORIZATION, self.authorization.clone());
        let builder = if kind == super::DraftKind::Delete {
            builder
        } else {
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(request.body().to_vec())
        };
        let response = builder
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        let status = response.status().as_u16();
        let rejected = match status {
            401 | 403 => Some(TransportError::Authentication),
            400 if kind != super::DraftKind::Delete => {
                Some(TransportError::Domain(Error::InvalidDraft))
            }
            404 if kind != super::DraftKind::Create => Some(TransportError::NotFound),
            _ => None,
        };
        if let Some(problem) = rejected {
            return Ok(super::change::WriteReply {
                outcome: super::WriteOutcome::Rejected,
                problem: Some(problem),
            });
        }
        let bytes = bounded_body(response).await?;
        let outcome = request.decode_success(status, &bytes);
        let problem = matches!(outcome, super::WriteOutcome::Unconfirmed)
            .then_some(TransportError::Domain(Error::InvalidResponse));
        Ok(super::change::WriteReply { outcome, problem })
    }
    /// Captures the immutable draft for future durable dispatch integration.
    /// This performs no HTTP write and is not itself a persisted ledger.
    pub fn capture(&self, draft: super::Draft) -> Result<super::WriteAttempt, Error> {
        Ok(super::WriteAttempt::new(super::WriteRequest::capture(
            self.scope, draft,
        )?))
    }
    pub fn write_scope(&self) -> super::WriteScope {
        self.scope
    }
    pub async fn project_billing_accounts(
        &self,
        project: super::JiraProjectId,
    ) -> Result<super::ProjectBillingAccounts, TransportError> {
        tokio::time::timeout(READ_TIMEOUT, async {
            let url = self
                .base
                .join(&format!("account-links/project/{}", project.get()))
                .map_err(|_| Error::InvalidResponse)?;
            let response = self
                .http
                .get(url)
                .header(header::AUTHORIZATION, self.authorization.clone())
                .query(&[("includeGlobalAccounts", "true")])
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            let links = super::project_accounts::ProjectLinks::parse(
                &bounded_body(response).await?,
                project,
            )?;
            let choices = links.resolve(self.billing_accounts().await?)?;
            Ok(super::ProjectBillingAccounts {
                connection: self.scope,
                project,
                choices,
            })
        })
        .await
        .map_err(|_| TransportError::Unavailable)?
    }
    /// Return a complete catalog or an error. Never expose partially loaded choices.
    pub async fn billing_accounts(&self) -> Result<Vec<super::BillingAccount>, TransportError> {
        let url = self
            .base
            .join("accounts")
            .map_err(|_| Error::InvalidResponse)?;
        tokio::time::timeout(READ_TIMEOUT, async {
            let mut pages = super::accounts::AccountPages::new();
            loop {
                let response = self
                    .http
                    .get(url.clone())
                    .header(header::AUTHORIZATION, self.authorization.clone())
                    .query(&[("limit", READ_PAGE_SIZE), ("offset", pages.offset())])
                    .send()
                    .await
                    .map_err(|_| TransportError::Unavailable)?;
                let (next, complete) = pages.push(&bounded_body(response).await?)?;
                pages = next;
                if complete {
                    return Ok(pages.finish());
                }
            }
        })
        .await
        .map_err(|_| TransportError::Unavailable)?
    }
    pub fn new(token: &str) -> Result<Self, TransportError> {
        Self::build(
            token,
            Url::parse(API).expect("static Tempo HTTPS origin"),
            true,
        )
    }
    pub(super) fn build(token: &str, base: Url, https: bool) -> Result<Self, TransportError> {
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
            scope: super::WriteScope::new(),
        })
    }
    /// Read a captured identity without accepting a server-provided URL.
    /// This is a read only; callers must still compare the observed revision
    /// before a future update and retain a draft on every failure.
    pub async fn worklog(
        &self,
        author: &AccountId,
        id: WorklogId,
    ) -> Result<Worklog, TransportError> {
        let url = self
            .base
            .join(&format!("worklogs/{}", id.get()))
            .map_err(|_| Error::InvalidResponse)?;
        let response = self
            .http
            .get(url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        let row = parse_worklog(&bounded_body(response).await?)?;
        if row.id != id || &row.author != author {
            return Err(Error::WrongScope.into());
        }
        Ok(row)
    }
    /// A complete baseline for editing. When attributes need a second request,
    /// compare the enclosing record again before combining the two responses.
    /// This detects observed changes; it is not an atomic server write lock.
    pub async fn worklog_for_edit(
        &self,
        author: &AccountId,
        id: WorklogId,
    ) -> Result<Worklog, TransportError> {
        tokio::time::timeout(READ_TIMEOUT, async {
            let row = self.worklog(author, id).await?;
            if row.attributes.is_some() {
                return Ok(row);
            }
            let url = self
                .base
                .join(&format!("worklogs/{}/work-attribute-values", id.get()))
                .map_err(|_| Error::InvalidResponse)?;
            let response = self
                .http
                .get(url)
                .header(header::AUTHORIZATION, self.authorization.clone())
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            let attributes = parse_worklog_attributes(&bounded_body(response).await?, id)?;
            let mut latest = self.worklog(author, id).await?;
            let loaded_attributes = latest.attributes.take();
            if latest != row {
                return Err(Error::ChangedWhileLoading.into());
            }
            latest.attributes = Some(loaded_attributes.unwrap_or(attributes));
            Ok(latest)
        })
        .await
        .map_err(|_| TransportError::Unavailable)?
    }
    pub async fn worklogs(
        &self,
        author: AccountId,
        range: DateRange,
    ) -> Result<Vec<Worklog>, TransportError> {
        let mut pages = ReadPages::new(author, range)?;
        let url = self
            .base
            .join(&format!("worklogs/user/{}", pages.author().as_str()))
            .map_err(|_| Error::InvalidResponse)?;
        let from = pages.range().start.to_string();
        let to = pages.range().end.to_string();
        tokio::time::timeout(READ_TIMEOUT, async {
            loop {
                let offset = pages.request_offset()?;
                let response = self
                    .http
                    .get(url.clone())
                    .header(header::AUTHORIZATION, self.authorization.clone())
                    .query(&[
                        ("from", from.as_str()),
                        ("to", to.as_str()),
                        ("limit", &READ_PAGE_SIZE.to_string()),
                        ("offset", &offset.to_string()),
                    ])
                    .send()
                    .await
                    .map_err(|_| TransportError::Unavailable)?;
                let bytes = bounded_body(response).await?;
                pages.push(offset, parse_worklogs(&bytes)?)?;
                if pages.is_complete() {
                    return pages.finish().map_err(Into::into);
                }
            }
        })
        .await
        .map_err(|_| TransportError::Unavailable)?
    }
    pub async fn schedule(
        &self,
        author: AccountId,
        range: DateRange,
    ) -> Result<Vec<DaySchedule>, TransportError> {
        let scope = ReadPages::new(author, range)?;
        let url = self
            .base
            .join(&format!("user-schedule/{}", scope.author().as_str()))
            .map_err(|_| Error::InvalidResponse)?;
        let response = self
            .http
            .get(url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .query(&[
                ("from", scope.range().start.to_string()),
                ("to", scope.range().end.to_string()),
            ])
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        let rows = parse_schedule(&bounded_body(response).await?)?;
        if rows.iter().any(|row| !scope.range().contains(row.date)) {
            return Err(Error::WrongScope.into());
        }
        Ok(rows)
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
    if status == StatusCode::NOT_FOUND {
        return Err(TransportError::NotFound);
    }
    if !status.is_success() {
        return Err(TransportError::Service);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::TooLarge.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| TransportError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(Error::TooLarge.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
pub(super) mod tests;
