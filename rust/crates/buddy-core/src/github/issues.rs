use serde_json::{Value, json};

use super::browsing::number;
use super::details::{actor, web_url};
use super::pulls::text;
use super::{ApiRequest, GitHubClient, GitHubError, Page, PageCursor, PullRef, RepositoryRef};

#[derive(Debug, Clone)]
pub struct IssueSummary {
    pub repository: RepositoryRef,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub comment_count: u64,
}

#[derive(Debug, Clone)]
pub struct IssueDetail {
    pub issue: IssueSummary,
    pub body: String,
    pub closed_at: Option<String>,
    pub state_reason: Option<String>,
    pub milestone: Option<String>,
    pub due_on: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IssueComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub url: Option<String>,
}

impl IssueSummary {
    pub fn web_url(&self) -> String {
        format!("{}/issues/{}", self.repository.web_url(), self.number)
    }

    fn parse(repository: &RepositoryRef, value: &Value) -> Result<Self, GitHubError> {
        let issue_number = number(value, "number")?;
        PullRef::new(repository.clone(), issue_number)?;
        Ok(Self {
            repository: repository.clone(),
            number: issue_number,
            title: text(value, "title")?.into(),
            state: text(value, "state")?.into(),
            author: actor(value),
            created_at: text(value, "created_at")?.into(),
            updated_at: text(value, "updated_at")?.into(),
            labels: value["labels"]
                .as_array()
                .ok_or(GitHubError::InvalidData)?
                .iter()
                .map(|v| text(v, "name").map(str::to_string))
                .collect::<Result<_, _>>()?,
            assignees: value["assignees"]
                .as_array()
                .ok_or(GitHubError::InvalidData)?
                .iter()
                .map(|v| text(v, "login").map(str::to_string))
                .collect::<Result<_, _>>()?,
            comment_count: number(value, "comments")?,
        })
    }
}

impl GitHubClient {
    pub async fn repository_issues(
        &self,
        repository: &RepositoryRef,
        closed: bool,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<IssueSummary>, GitHubError> {
        let state = if closed { "closed" } else { "open" };
        let page = self
            .rest_page(
                &format!(
                    "{}/issues?state={state}&sort=updated&direction=desc",
                    repository.api_path()
                ),
                cursor,
                None,
                force,
            )
            .await?;
        // The REST issues collection also contains PRs. Keep its pagination
        // even when a filtered page contains no actual issues.
        Ok(Page {
            items: page
                .items
                .into_iter()
                .filter(|value| value.get("pull_request").is_none())
                .map(|value| IssueSummary::parse(repository, &value))
                .collect::<Result<_, _>>()?,
            next: page.next,
            truncated: page.truncated,
        })
    }

    pub async fn issue(
        &self,
        repository: &RepositoryRef,
        issue_number: u64,
        force: bool,
    ) -> Result<IssueDetail, GitHubError> {
        PullRef::new(repository.clone(), issue_number)?;
        let response = self
            .request(
                ApiRequest::get(format!("{}/issues/{issue_number}", repository.api_path())),
                force,
            )
            .await?
            .value;
        let value = &response.body;
        if value.get("pull_request").is_some() {
            return Err(GitHubError::InvalidResource);
        }
        let issue = IssueSummary::parse(repository, value)?;
        if issue.number != issue_number {
            return Err(GitHubError::InvalidData);
        }
        Ok(IssueDetail {
            issue,
            body: value["body"].as_str().unwrap_or("").into(),
            closed_at: value["closed_at"].as_str().map(Into::into),
            state_reason: value["state_reason"].as_str().map(Into::into),
            milestone: value["milestone"]["title"].as_str().map(Into::into),
            due_on: value["milestone"]["due_on"].as_str().map(Into::into),
        })
    }

    pub async fn issue_comments(
        &self,
        repository: &RepositoryRef,
        issue_number: u64,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<IssueComment>, GitHubError> {
        PullRef::new(repository.clone(), issue_number)?;
        self.rest_page(
            &format!("{}/issues/{issue_number}/comments", repository.api_path()),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| {
            Ok(IssueComment {
                id: number(&value, "id")?.to_string(),
                author: actor(&value),
                body: text(&value, "body")?.into(),
                created_at: text(&value, "created_at")?.into(),
                updated_at: text(&value, "updated_at")?.into(),
                url: web_url(&value["html_url"]),
            })
        })
    }

    pub async fn repository_counts(
        &self,
        repository: &RepositoryRef,
        force: bool,
    ) -> Result<(u64, u64), GitHubError> {
        let query = "query NativeRepositoryCounts($owner:String!,$repo:String!) {repository(owner:$owner,name:$repo) {issues(states:OPEN,first:1) {totalCount} pullRequests(states:OPEN,first:1) {totalCount}}}";
        let response = self
            .request(
                ApiRequest::graphql(
                    query,
                    json!({"owner":repository.owner(),"repo":repository.name()}),
                    false,
                ),
                force,
            )
            .await?
            .value;
        let value = &response.body["data"]["repository"];
        if value.is_null() {
            return Err(GitHubError::NotFound);
        }
        Ok((
            number(&value["issues"], "totalCount")?,
            number(&value["pullRequests"], "totalCount")?,
        ))
    }
}
