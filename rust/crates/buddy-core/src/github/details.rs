use serde_json::{Value, json};

use super::pagination::{Page, PageCursor};
use super::pulls::{PULL_FRAGMENT, PULL_LIST_FRAGMENT, text};
use super::{ApiRequest, GitHubClient, GitHubError, PullRef, PullSummary, RepositoryRef};

#[derive(Debug, Clone)]
pub struct CommentRef {
    pub(crate) pull: PullRef,
    pub(crate) id: String,
}

impl CommentRef {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn pull(&self) -> &PullRef {
        &self.pull
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub reference: CommentRef,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub url: Option<String>,
    pub diff_hunk: Option<String>,
    pub reactions: Vec<(String, u64)>,
}

impl Comment {
    pub(crate) fn parse(pull: &PullRef, value: &Value) -> Result<Self, GitHubError> {
        let id = text(value, "id")?;
        valid_node_id(id)?;
        let groups = value["reactionGroups"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?;
        let reactions = groups
            .iter()
            .map(|group| {
                Ok((
                    text(group, "content")?.to_string(),
                    group["users"]["totalCount"]
                        .as_u64()
                        .ok_or(GitHubError::InvalidData)?,
                ))
            })
            .collect::<Result<_, GitHubError>>()?;
        Ok(Self {
            reference: CommentRef {
                pull: pull.clone(),
                id: id.into(),
            },
            author: actor(value),
            body: text(value, "body")?.into(),
            created_at: text(value, "createdAt")?.into(),
            updated_at: text(value, "updatedAt")?.into(),
            url: web_url(&value["url"]),
            diff_hunk: value["diffHunk"].as_str().map(Into::into),
            reactions,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Review {
    pub id: String,
    pub author: String,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub repository: RepositoryRef,
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub changes: u64,
    /// GitHub omits patches for binary or sufficiently large diffs.
    pub patch: Option<String>,
    pub url: Option<String>,
}

impl FileDiff {
    pub(crate) fn parse(value: &Value) -> Result<Self, GitHubError> {
        Ok(Self {
            filename: text(value, "filename")?.into(),
            previous_filename: value["previous_filename"].as_str().map(Into::into),
            status: text(value, "status")?.into(),
            additions: value["additions"]
                .as_u64()
                .ok_or(GitHubError::InvalidData)?,
            deletions: value["deletions"]
                .as_u64()
                .ok_or(GitHubError::InvalidData)?,
            changes: value["changes"].as_u64().ok_or(GitHubError::InvalidData)?,
            patch: value["patch"].as_str().map(Into::into),
            url: web_url(&value["blob_url"]),
        })
    }
}

pub(crate) fn valid_node_id(id: &str) -> Result<(), GitHubError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        Err(GitHubError::InvalidResource)
    } else {
        Ok(())
    }
}

pub(crate) fn valid_sha(sha: &str) -> Result<(), GitHubError> {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        Err(GitHubError::InvalidResource)
    } else {
        Ok(())
    }
}

pub(crate) fn actor(value: &Value) -> String {
    value["author"]["login"]
        .as_str()
        .or_else(|| value["user"]["login"].as_str())
        .or_else(|| value["author"]["name"].as_str())
        .unwrap_or("Deleted account")
        .into()
}

pub(crate) fn web_url(value: &Value) -> Option<String> {
    let raw = value.as_str()?;
    let url = reqwest::Url::parse(raw).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| raw.to_string())
}

impl GitHubClient {
    pub(crate) fn pull_variables(&self, pull: &PullRef) -> Result<Value, GitHubError> {
        PullRef::new(pull.repository.clone(), pull.number)?;
        Ok(
            json!({"owner":pull.repository.owner(),"repo":pull.repository.name(),"number":pull.number,"reviewer":self.account.username()}),
        )
    }

    pub async fn pull(&self, pull: &PullRef, force: bool) -> Result<PullSummary, GitHubError> {
        let variables = self.pull_variables(pull)?;
        let query = format!(
            "query NativePull($owner:String!,$repo:String!,$number:Int!,$reviewer:String!) {{repository(owner:$owner,name:$repo) {{pullRequest(number:$number) {{...NativePullSummary}}}}}}\n{PULL_FRAGMENT}\n{PULL_LIST_FRAGMENT}"
        );
        let response = self
            .request(ApiRequest::graphql(&query, variables, false), force)
            .await?
            .value;
        let value = &response.body["data"]["repository"]["pullRequest"];
        if value.is_null() {
            return Err(GitHubError::NotFound);
        }
        let summary = PullSummary::from_graphql(value)?;
        if summary.body.is_none() {
            return Err(GitHubError::InvalidData);
        }
        if summary.target != *pull {
            return Err(GitHubError::InvalidData);
        }
        valid_sha(&summary.head_sha)?;
        Ok(summary)
    }

    pub async fn pull_files(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<FileDiff>, GitHubError> {
        self.pull_variables(pull)?;
        self.rest_page(
            &format!("{}/pulls/{}/files", pull.repository.api_path(), pull.number),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| FileDiff::parse(&value))
    }

    pub async fn pull_comments(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Comment>, GitHubError> {
        let query = r#"query NativePullComments($owner:String!,$repo:String!,$number:Int!,$after:String) {
         repository(owner:$owner,name:$repo) {pullRequest(number:$number) {
          comments(first:100,after:$after) {pageInfo {hasNextPage endCursor} nodes {
           id body createdAt updatedAt url author {login} reactionGroups {content users {totalCount}}
          }}
         }}
        }"#;
        let mut variables = self.pull_variables(pull)?;
        variables.as_object_mut().unwrap().remove("reviewer");
        self.connection(
            query,
            variables,
            "/data/repository/pullRequest/comments",
            cursor,
            force,
        )
        .await?
        .map(|value| Comment::parse(pull, &value))
    }

    pub async fn pull_reviews(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Review>, GitHubError> {
        let query = r#"query NativePullReviews($owner:String!,$repo:String!,$number:Int!,$after:String) {
         repository(owner:$owner,name:$repo) {pullRequest(number:$number) {
          reviews(first:100,after:$after) {pageInfo {hasNextPage endCursor} nodes {
           id body state submittedAt url author {login}
          }}
         }}
        }"#;
        let mut variables = self.pull_variables(pull)?;
        variables.as_object_mut().unwrap().remove("reviewer");
        self.connection(
            query,
            variables,
            "/data/repository/pullRequest/reviews",
            cursor,
            force,
        )
        .await?
        .map(|value| {
            Ok(Review {
                id: text(&value, "id")?.into(),
                author: actor(&value),
                state: text(&value, "state")?.into(),
                body: value["body"].as_str().unwrap_or("").into(),
                submitted_at: value["submittedAt"].as_str().map(Into::into),
                url: web_url(&value["url"]),
            })
        })
    }

    pub async fn pull_commits(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Commit>, GitHubError> {
        // The REST PR-commits endpoint caps its output; this connection retains
        // explicit pagination for larger pull requests.
        let query = r#"query NativePullCommits($owner:String!,$repo:String!,$number:Int!,$after:String) {
         repository(owner:$owner,name:$repo) {pullRequest(number:$number) {
          commits(first:100,after:$after) {pageInfo {hasNextPage endCursor} nodes {commit {
           oid message committedDate url author {name user {login}}
          }}}
         }}
        }"#;
        let mut variables = self.pull_variables(pull)?;
        variables.as_object_mut().unwrap().remove("reviewer");
        self.connection(
            query,
            variables,
            "/data/repository/pullRequest/commits",
            cursor,
            force,
        )
        .await?
        .map(|value| {
            let commit = &value["commit"];
            let sha = text(commit, "oid")?;
            valid_sha(sha)?;
            Ok(Commit {
                repository: pull.repository.clone(),
                sha: sha.into(),
                message: text(commit, "message")?.into(),
                date: text(commit, "committedDate")?.into(),
                author: commit["author"]["user"]["login"]
                    .as_str()
                    .or_else(|| commit["author"]["name"].as_str())
                    .unwrap_or("Unknown author")
                    .into(),
                url: web_url(&commit["url"]),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_ci_links_keep_http_navigation_without_accepting_executable_or_credential_urls() {
        for url in [
            "https://sonarcloud.io/project/overview?id=fixture",
            "https://github.com/team/repo",
            "http://docs.example.test:8080/project",
        ] {
            assert_eq!(web_url(&json!(url)).as_deref(), Some(url));
        }
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://user:secret@example.test/project",
            "https://user@example.test/project",
            "data:text/html,unsafe",
        ] {
            assert!(web_url(&json!(url)).is_none());
        }
    }

    #[test]
    fn missing_binary_or_large_patches_remain_unavailable_and_do_not_become_empty_diffs() {
        let file=FileDiff::parse(&json!({"filename":"assets/image.png","status":"added","additions":0,"deletions":0,"changes":0,"blob_url":"https://github.com/team/repo/blob/main/assets/image.png"})).unwrap();
        assert_eq!(file.patch, None);
        assert_eq!(file.filename, "assets/image.png");
        assert!(file.url.is_some());
    }
}
