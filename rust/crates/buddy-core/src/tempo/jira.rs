//! Connection-local Jira reads. No identity cache, credential fallback, or writes.
use std::time::Duration;

use reqwest::{StatusCode, Url};
use serde::Deserialize;

use super::{AccountId, IssueId, MAX_RESPONSE_BYTES, bounded};

#[path = "capitalization.rs"]
mod capitalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraError {
    InvalidConnection,
    InvalidIssue,
    Unavailable,
    Authentication,
    RateLimited,
    NotFound,
    Redirect,
    InvalidResponse,
    TooLarge,
    WrongIssue,
    InvalidProject,
    WrongProject,
}
impl std::fmt::Display for JiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConnection => "Enter a Jira HTTPS origin, email, and API token.",
            Self::InvalidIssue => "Enter a Jira issue key or numeric issue ID.",
            Self::Unavailable => {
                "Could not load Jira data. The existing worklog draft is retained."
            }
            Self::Authentication => {
                "Jira rejected this connection. Check its credentials before refreshing."
            }
            Self::RateLimited => "Jira is limiting requests. Wait before refreshing.",
            Self::NotFound => "This Jira issue is unavailable. Check its key and your access.",
            Self::Redirect => {
                "Jira returned an unexpected redirect. This connection did not follow it."
            }
            Self::InvalidResponse => {
                "Jira returned unsupported data. The existing worklog draft is retained."
            }
            Self::TooLarge => "The Jira response exceeds the supported size.",
            Self::WrongIssue => {
                "Jira returned a different issue. Check the issue key before saving."
            }
            Self::InvalidProject => "Enter a Jira project key or positive numeric project ID.",
            Self::WrongProject => {
                "Jira returned a different project. Existing billing choices are retained."
            }
        })
    }
}
impl std::error::Error for JiraError {}

/// A missing zone stays missing. Resolve the returned IANA zone before local
/// date/time conversion; never substitute the machine's current zone.
pub struct JiraProfile {
    pub account: AccountId,
    pub time_zone: Option<String>,
}
pub struct JiraIssue {
    pub id: IssueId,
    pub key: String,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct JiraProjectId(u64);
impl JiraProjectId {
    pub fn parse(value: u64) -> Result<Self, JiraError> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(JiraError::InvalidProject);
        }
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
pub struct JiraProject {
    pub id: JiraProjectId,
    pub key: String,
}

fn project_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// No Debug: credentials and private response data stay out of diagnostics.
pub struct JiraClient {
    http: reqwest::Client,
    origin: Url,
    email: String,
    token: String,
}

fn valid_parts(origin: &Url, email: &str, token: &str) -> bool {
    origin.host_str().is_some()
        && origin.path() == "/"
        && origin.username().is_empty()
        && origin.password().is_none()
        && origin.query().is_none()
        && origin.fragment().is_none()
        && !email.trim().is_empty()
        && !email.contains(':')
        && bounded(email, 320, false)
        && !token.is_empty()
        && token.len() <= 8192
        && token.bytes().all(|byte| byte.is_ascii_graphic())
}

pub(super) fn connection_origin(origin: &str, email: &str, token: &str) -> Result<Url, JiraError> {
    if !bounded(origin, 2048, false) {
        return Err(JiraError::InvalidConnection);
    }
    let origin = Url::parse(origin).map_err(|_| JiraError::InvalidConnection)?;
    if origin.scheme() != "https" || !valid_parts(&origin, email, token) {
        return Err(JiraError::InvalidConnection);
    }
    Ok(origin)
}

impl JiraClient {
    pub fn new(origin: &str, email: &str, token: &str) -> Result<Self, JiraError> {
        Self::build(connection_origin(origin, email, token)?, email, token, true)
    }
    pub(super) fn build(
        origin: Url,
        email: &str,
        token: &str,
        https: bool,
    ) -> Result<Self, JiraError> {
        if !valid_parts(&origin, email, token) {
            return Err(JiraError::InvalidConnection);
        }
        let builder = reqwest::Client::builder()
            .user_agent("hs-buddy/1.0")
            .https_only(https)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(15));
        let builder = if https { builder } else { builder.no_proxy() };
        Ok(Self {
            http: builder.build().map_err(|_| JiraError::Unavailable)?,
            origin,
            email: email.into(),
            token: token.into(),
        })
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    pub async fn project(&self, key_or_id: &str) -> Result<JiraProject, JiraError> {
        let numeric = !key_or_id.is_empty() && key_or_id.bytes().all(|byte| byte.is_ascii_digit());
        if !(numeric || project_key(key_or_id)) || key_or_id.len() > 256 {
            return Err(JiraError::InvalidProject);
        }
        let requested_id = if numeric {
            Some(JiraProjectId::parse(
                key_or_id.parse().map_err(|_| JiraError::InvalidProject)?,
            )?)
        } else {
            None
        };
        #[derive(Deserialize)]
        struct RawProject {
            id: String,
            key: String,
        }
        let raw: RawProject =
            serde_json::from_slice(&self.get(&format!("rest/api/3/project/{key_or_id}")).await?)
                .map_err(|_| JiraError::InvalidResponse)?;
        if raw.id.is_empty()
            || !raw.id.bytes().all(|byte| byte.is_ascii_digit())
            || !project_key(&raw.key)
        {
            return Err(JiraError::InvalidResponse);
        }
        let id = JiraProjectId::parse(raw.id.parse().map_err(|_| JiraError::InvalidResponse)?)
            .map_err(|_| JiraError::InvalidResponse)?;
        if requested_id.is_some_and(|expected| id != expected) || (!numeric && raw.key != key_or_id)
        {
            return Err(JiraError::WrongProject);
        }
        Ok(JiraProject { id, key: raw.key })
    }
    async fn get(&self, path: &str) -> Result<Vec<u8>, JiraError> {
        let url = self
            .origin
            .join(path)
            .map_err(|_| JiraError::InvalidConnection)?;
        let mut response = self
            .http
            .get(url)
            .basic_auth(&self.email, Some(&self.token))
            .send()
            .await
            .map_err(|_| JiraError::Unavailable)?;
        let status = response.status();
        if status.is_redirection() {
            return Err(JiraError::Redirect);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(JiraError::Authentication);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(JiraError::RateLimited);
        }
        if status == StatusCode::NOT_FOUND {
            return Err(JiraError::NotFound);
        }
        if !status.is_success() {
            return Err(JiraError::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(JiraError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| JiraError::Unavailable)? {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(JiraError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn myself(&self) -> Result<JiraProfile, JiraError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Profile {
            account_id: String,
            time_zone: Option<String>,
        }
        let raw: Profile = serde_json::from_slice(&self.get("rest/api/3/myself").await?)
            .map_err(|_| JiraError::InvalidResponse)?;
        if raw
            .time_zone
            .as_ref()
            .is_some_and(|zone| zone.trim().is_empty() || !bounded(zone, 256, false))
        {
            return Err(JiraError::InvalidResponse);
        }
        Ok(JiraProfile {
            account: AccountId::parse(&raw.account_id).map_err(|_| JiraError::InvalidResponse)?,
            time_zone: raw.time_zone,
        })
    }
    pub async fn issue(&self, key_or_id: &str) -> Result<JiraIssue, JiraError> {
        if !identifier(key_or_id) {
            return Err(JiraError::InvalidIssue);
        }
        #[derive(Deserialize)]
        struct Fields {
            summary: Option<String>,
        }
        #[derive(Deserialize)]
        struct Issue {
            id: String,
            key: String,
            fields: Option<Fields>,
        }
        let path = format!("rest/api/3/issue/{key_or_id}?fields=id,key,summary");
        let raw: Issue = serde_json::from_slice(&self.get(&path).await?)
            .map_err(|_| JiraError::InvalidResponse)?;
        let id = issue_identity(key_or_id, &raw.id, &raw.key)?;
        let summary = raw.fields.and_then(|fields| fields.summary);
        if summary
            .as_ref()
            .is_some_and(|text| !bounded(text, 16 * 1024, false))
        {
            return Err(JiraError::InvalidResponse);
        }
        Ok(JiraIssue {
            id,
            key: raw.key,
            summary,
        })
    }
}
fn issue_identity(requested: &str, raw_id: &str, key: &str) -> Result<IssueId, JiraError> {
    if raw_id.is_empty() || !raw_id.bytes().all(|byte| byte.is_ascii_digit()) || !identifier(key) {
        return Err(JiraError::InvalidResponse);
    }
    let id = IssueId::parse(raw_id.parse().map_err(|_| JiraError::InvalidResponse)?)
        .map_err(|_| JiraError::InvalidResponse)?;
    let numeric = requested.bytes().all(|byte| byte.is_ascii_digit());
    if (numeric && requested.parse::<u64>().ok() != Some(id.get()))
        || (!numeric && !key.eq_ignore_ascii_case(requested))
    {
        return Err(JiraError::WrongIssue);
    }
    Ok(id)
}
fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 256
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
#[path = "jira_tests.rs"]
pub(super) mod tests;
