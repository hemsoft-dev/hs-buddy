use serde_json::Value;

use super::details::{valid_sha, web_url};
use super::pulls::text;
use super::{
    ApiRequest, Commit, FileDiff, GitHubClient, GitHubError, Page, PageCursor, RepositoryRef,
};

#[derive(Debug, Clone)]
pub struct RepositorySummary {
    pub target: RepositoryRef,
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub default_branch: Option<String>,
    pub visibility: String,
    pub archived: bool,
    pub fork: bool,
    pub stars: u64,
    pub forks: u64,
    pub watchers: Option<u64>,
    /// GitHub's REST count includes pull requests. Do not label it "issues".
    pub open_items: u64,
    pub size_kb: u64,
    pub updated_at: String,
    pub pushed_at: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub topics: Vec<String>,
}

impl RepositorySummary {
    pub(crate) fn parse(value: &Value) -> Result<Self, GitHubError> {
        let full_name = text(value, "full_name")?.to_string();
        let private = value["private"].as_bool().ok_or(GitHubError::InvalidData)?;
        Ok(Self {
            target: RepositoryRef::parse(&full_name)?,
            full_name,
            description: value["description"].as_str().map(Into::into),
            language: value["language"].as_str().map(Into::into),
            default_branch: value["default_branch"].as_str().map(Into::into),
            visibility: value["visibility"]
                .as_str()
                .unwrap_or(if private { "private" } else { "public" })
                .into(),
            archived: value["archived"]
                .as_bool()
                .ok_or(GitHubError::InvalidData)?,
            fork: value["fork"].as_bool().ok_or(GitHubError::InvalidData)?,
            stars: number(value, "stargazers_count")?,
            forks: number(value, "forks_count")?,
            watchers: value["subscribers_count"].as_u64(),
            open_items: number(value, "open_issues_count")?,
            size_kb: number(value, "size")?,
            updated_at: text(value, "updated_at")?.into(),
            pushed_at: value["pushed_at"].as_str().map(Into::into),
            homepage: web_url(&value["homepage"]),
            license: value["license"]["name"].as_str().map(Into::into),
            topics: value["topics"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(Into::into)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct UserSummary {
    pub login: String,
    pub name: Option<String>,
    pub kind: String,
    pub url: Option<String>,
}

impl UserSummary {
    pub(crate) fn parse(value: &Value) -> Result<Self, GitHubError> {
        let login = text(value, "login")?;
        valid_login(login)?;
        Ok(Self {
            login: login.into(),
            name: value["name"].as_str().map(Into::into),
            kind: text(value, "type")?.into(),
            url: web_url(&value["html_url"]),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TeamSummary {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub members: Option<u64>,
    pub repositories: Option<u64>,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub commit: Commit,
    pub parents: Vec<String>,
    pub additions: u64,
    pub deletions: u64,
    pub total: u64,
}

pub(crate) fn valid_login(login: &str) -> Result<(), GitHubError> {
    let base = login.strip_suffix("[bot]").unwrap_or(login);
    if crate::gh::is_valid_slug(base) {
        Ok(())
    } else {
        Err(GitHubError::InvalidResource)
    }
}

pub(crate) fn number(value: &Value, key: &str) -> Result<u64, GitHubError> {
    value[key].as_u64().ok_or(GitHubError::InvalidData)
}

impl Commit {
    pub(crate) fn from_rest(
        repository: &RepositoryRef,
        value: &Value,
    ) -> Result<Self, GitHubError> {
        let sha = text(value, "sha")?;
        valid_sha(sha)?;
        Ok(Self {
            repository: repository.clone(),
            sha: sha.into(),
            message: text(&value["commit"], "message")?.into(),
            author: value["author"]["login"]
                .as_str()
                .or_else(|| value["commit"]["author"]["name"].as_str())
                .unwrap_or("Unknown author")
                .into(),
            date: text(&value["commit"]["committer"], "date")?.into(),
            url: web_url(&value["html_url"]),
        })
    }
}

impl GitHubClient {
    pub async fn repositories(
        &self,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<RepositorySummary>, GitHubError> {
        let personal = self.namespace_is_personal(force).await?;
        let path = if personal {
            // `/users/name/repos` omits private repositories. Enumerate this
            // selected account's accessible repositories and filter the owner.
            "/user/repos?visibility=all&affiliation=owner,collaborator&sort=updated&direction=desc"
                .to_string()
        } else {
            format!(
                "/orgs/{}/repos?type=all&sort=updated&direction=desc",
                self.account.organization()
            )
        };
        let page = self
            .rest_page(&path, cursor, None, force)
            .await?
            .map(|value| RepositorySummary::parse(&value))?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .filter(|repository| {
                    repository
                        .target
                        .owner()
                        .eq_ignore_ascii_case(self.account.organization())
                })
                .collect(),
            next: page.next,
            truncated: page.truncated,
        })
    }

    pub async fn repository(
        &self,
        repository: &RepositoryRef,
        force: bool,
    ) -> Result<RepositorySummary, GitHubError> {
        let response = self
            .request(ApiRequest::get(repository.api_path()), force)
            .await?
            .value;
        let summary = RepositorySummary::parse(&response.body)?;
        if summary.target != *repository {
            return Err(GitHubError::InvalidData);
        }
        Ok(summary)
    }

    pub async fn repository_commits(
        &self,
        repository: &RepositoryRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Commit>, GitHubError> {
        let page = match self
            .rest_page(
                &format!("{}/commits", repository.api_path()),
                cursor,
                None,
                force,
            )
            .await
        {
            Ok(page) => page,
            Err(GitHubError::EmptyRepository) => {
                return Ok(Page {
                    items: Vec::new(),
                    next: None,
                    truncated: false,
                });
            }
            Err(error) => return Err(error),
        };
        page.map(|value| Commit::from_rest(repository, &value))
    }

    pub async fn commit(
        &self,
        repository: &RepositoryRef,
        sha: &str,
        force: bool,
    ) -> Result<CommitDetail, GitHubError> {
        valid_sha(sha)?;
        let response = self
            .request(
                ApiRequest::get(format!(
                    "{}/commits/{sha}?per_page=1",
                    repository.api_path()
                )),
                force,
            )
            .await?
            .value;
        let value = &response.body;
        let commit = Commit::from_rest(repository, value)?;
        if !commit.sha.eq_ignore_ascii_case(sha) {
            return Err(GitHubError::InvalidData);
        }
        let parents = value["parents"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?
            .iter()
            .map(|parent| {
                let sha = text(parent, "sha")?;
                valid_sha(sha)?;
                Ok(sha.to_string())
            })
            .collect::<Result<_, GitHubError>>()?;
        Ok(CommitDetail {
            commit,
            parents,
            additions: number(&value["stats"], "additions")?,
            deletions: number(&value["stats"], "deletions")?,
            total: number(&value["stats"], "total")?,
        })
    }

    pub async fn commit_files(
        &self,
        repository: &RepositoryRef,
        sha: &str,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<FileDiff>, GitHubError> {
        valid_sha(sha)?;
        self.rest_page(
            &format!("{}/commits/{sha}", repository.api_path()),
            cursor,
            Some("files"),
            force,
        )
        .await?
        .map(|value| FileDiff::parse(&value))
    }

    pub async fn organization_members(
        &self,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<UserSummary>, GitHubError> {
        if self.namespace_is_personal(force).await? {
            return Ok(Page {
                items: Vec::new(),
                next: None,
                truncated: false,
            });
        }
        self.rest_page(
            &format!("/orgs/{}/members?role=all", self.account.organization()),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| UserSummary::parse(&value))
    }
}
