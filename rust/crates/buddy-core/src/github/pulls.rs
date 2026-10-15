use std::collections::{BTreeMap, HashSet};

use chrono::{Days, NaiveDate};
use serde_json::{Value, json};

use super::{ApiRequest, GitHubClient, GitHubError, PrMode, PullRef, RepositoryRef};

const MAX_SEARCH_PAGES: usize = 50;
pub(crate) const PULL_LIST_FRAGMENT: &str = r#"
fragment NativePullList on PullRequest {
   number title state createdAt updatedAt mergedAt headRefName baseRefName headRefOid
   changedFiles isDraft reviewDecision author {login} repository {nameWithOwner}
   latestOpinion:reviews(last:1,author:$reviewer) {nodes {state}}
   reviewThreads(first:1) {totalCount}
}
"#;
pub(crate) const PULL_FRAGMENT: &str = r#"
fragment NativePullSummary on PullRequest {
 ...NativePullList body
 recentReviews:reviews(last:100) {nodes {state author {login}} pageInfo {hasPreviousPage}}
}
"#;
const SEARCH_QUERY: &str = r#"
query NativePullRequests($query:String!,$reviewer:String!,$after:String) {
 search(query:$query,type:ISSUE,first:20,after:$after) {
  issueCount pageInfo {hasNextPage endCursor}
  nodes { ...NativePullList }
 }
}
"#;

#[derive(Debug, Clone)]
pub struct PullSummary {
    pub target: PullRef,
    pub title: String,
    pub author: String,
    /// List queries omit the description; Some("") is a loaded empty body.
    pub body: Option<String>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub draft: bool,
    pub i_approved: bool,
    /// None means historical reviews exceed the fully retrieved window.
    pub approval_count: Option<usize>,
    pub review_decision: Option<String>,
    pub thread_count: u64,
    pub changed_files: u64,
}

impl PullSummary {
    pub fn matches_filter(&self, text: &str) -> bool {
        let text = text.trim().to_lowercase();
        text.is_empty()
            || [
                &self.title,
                &self.author,
                &self.head_branch,
                &self.base_branch,
                &self.target.repository.full_name(),
                &format!("#{}", self.target.number),
            ]
            .iter()
            .any(|value| value.to_lowercase().contains(&text))
    }

    pub(crate) fn from_graphql(value: &Value) -> Result<Self, GitHubError> {
        let repository = RepositoryRef::parse(text(&value["repository"], "nameWithOwner")?)?;
        let target = PullRef::new(
            repository,
            value["number"].as_u64().ok_or(GitHubError::InvalidData)?,
        )?;
        let opinions = value["latestOpinion"]["nodes"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?;
        let i_approved = opinions.last().is_some_and(|v| v["state"] == "APPROVED");
        let approval_count = approval_count(value.get("recentReviews"))?;
        Ok(Self {
            target,
            title: text(value, "title")?.into(),
            author: value["author"]["login"]
                .as_str()
                .unwrap_or("Deleted account")
                .into(),
            body: value
                .get("body")
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or(GitHubError::InvalidData)
                })
                .transpose()?,
            state: text(value, "state")?.into(),
            created_at: text(value, "createdAt")?.into(),
            updated_at: text(value, "updatedAt")?.into(),
            merged_at: value["mergedAt"].as_str().map(Into::into),
            head_branch: text(value, "headRefName")?.into(),
            base_branch: text(value, "baseRefName")?.into(),
            head_sha: text(value, "headRefOid")?.into(),
            draft: value["isDraft"].as_bool().ok_or(GitHubError::InvalidData)?,
            i_approved,
            approval_count,
            review_decision: value["reviewDecision"].as_str().map(Into::into),
            thread_count: value["reviewThreads"]["totalCount"]
                .as_u64()
                .ok_or(GitHubError::InvalidData)?,
            changed_files: value["changedFiles"]
                .as_u64()
                .ok_or(GitHubError::InvalidData)?,
        })
    }
}

fn approval_count(connection: Option<&Value>) -> Result<Option<usize>, GitHubError> {
    let Some(connection) = connection else {
        return Ok(None);
    };
    let reviews = connection["nodes"]
        .as_array()
        .ok_or(GitHubError::InvalidData)?;
    let previous = connection["pageInfo"]["hasPreviousPage"]
        .as_bool()
        .ok_or(GitHubError::InvalidData)?;
    if previous {
        return Ok(None);
    }
    let mut latest = BTreeMap::new();
    for review in reviews {
        if let Some(author) = review["author"]["login"].as_str() {
            latest.insert(author.to_ascii_lowercase(), text(review, "state")?);
        }
    }
    Ok(Some(
        latest
            .values()
            .filter(|state| **state == "APPROVED")
            .count(),
    ))
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, GitHubError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(GitHubError::InvalidData)
}

#[derive(Debug)]
pub struct PullSearchResult {
    pub items: Vec<PullSummary>,
    pub truncated: bool,
    /// A successful source must never hide a failed source in multi-query modes.
    pub warnings: Vec<GitHubError>,
}

impl GitHubClient {
    pub async fn pull_requests(
        &self,
        mode: PrMode,
        merged_days: u32,
        force: bool,
    ) -> Result<PullSearchResult, GitHubError> {
        self.pull_requests_at(mode, merged_days, chrono::Utc::now().date_naive(), force)
            .await
    }

    pub async fn pull_requests_at(
        &self,
        mode: PrMode,
        merged_days: u32,
        today: NaiveDate,
        force: bool,
    ) -> Result<PullSearchResult, GitHubError> {
        let merged_after = today
            .checked_sub_days(Days::new(u64::from(merged_days.clamp(1, 3650))))
            .ok_or(GitHubError::InvalidData)?
            .format("%Y-%m-%d")
            .to_string();
        let mut result = PullSearchResult {
            items: Vec::new(),
            truncated: false,
            warnings: Vec::new(),
        };
        let mut unique = BTreeMap::new();
        let mut successful_sources = 0;
        let personal = self.namespace_is_personal(force).await?;
        for query in mode.searches_in_namespace(&self.account, &merged_after, personal) {
            match self
                .search_pulls(&format!("{query} sort:updated-desc"), force)
                .await
            {
                Ok((pulls, truncated)) => {
                    successful_sources += 1;
                    result.truncated |= truncated;
                    for pull in pulls {
                        // A later source can contain the newer review state.
                        unique.insert(pull.target.clone(), pull);
                    }
                }
                Err(error) => {
                    let limited = matches!(error, GitHubError::RateLimited { .. });
                    result.warnings.push(error);
                    if limited {
                        break;
                    }
                }
            }
        }
        if successful_sources == 0 {
            return Err(result
                .warnings
                .first()
                .cloned()
                .unwrap_or(GitHubError::InvalidData));
        }
        result.items = unique
            .into_values()
            .filter(|pull| mode.includes_review(pull.i_approved))
            .collect();
        result.items.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.target.cmp(&right.target))
        });
        Ok(result)
    }

    pub async fn namespace_is_personal(&self, force: bool) -> Result<bool, GitHubError> {
        let response = self
            .request(
                ApiRequest::get(format!("/users/{}", self.account.organization())),
                force,
            )
            .await?;
        match response.value.body["type"].as_str() {
            Some("User") => Ok(true),
            Some("Organization") => Ok(false),
            _ => Err(GitHubError::InvalidData),
        }
    }

    pub(crate) async fn search_pulls(
        &self,
        query: &str,
        force: bool,
    ) -> Result<(Vec<PullSummary>, bool), GitHubError> {
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        let mut truncated = false;
        for page in 0..MAX_SEARCH_PAGES {
            let response = self
                .request(
                    ApiRequest::graphql(
                        &format!("{SEARCH_QUERY}\n{PULL_LIST_FRAGMENT}"),
                        json!({"query":query,"reviewer":self.account.username(),"after":cursor}),
                        false,
                    ),
                    force,
                )
                .await?;
            let search = &response.value.body["data"]["search"];
            let total = search["issueCount"]
                .as_u64()
                .ok_or(GitHubError::InvalidData)?;
            truncated |= total > 1000;
            let nodes = search["nodes"].as_array().ok_or(GitHubError::InvalidData)?;
            for node in nodes {
                items.push(PullSummary::from_graphql(node)?);
            }
            let more = search["pageInfo"]["hasNextPage"]
                .as_bool()
                .ok_or(GitHubError::InvalidData)?;
            if !more {
                return Ok((items, truncated));
            }
            let next = search["pageInfo"]["endCursor"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
                .ok_or(GitHubError::InvalidData)?;
            if !seen.insert(next.to_string()) {
                return Err(GitHubError::InvalidData);
            }
            cursor = Some(next.into());
            if page + 1 == MAX_SEARCH_PAGES {
                truncated = true;
            }
        }
        Ok((items, truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AccountScope, ApiResponse, GitHubClients, Transport};
    use super::*;
    use std::sync::{Arc, Mutex};

    fn node(number: u64, state: &str) -> Value {
        json!({"number":number,"repository":{"nameWithOwner":"team/repo"},"title":format!("Real fixture PR {number}"),"body":"The PR body","author":{"login":"contributor"},"state":"OPEN","createdAt":"2026-10-01T00:00:00Z","updatedAt":"2026-10-15T00:00:00Z","mergedAt":null,"headRefName":"feature/fixture","baseRefName":"main","headRefOid":"0123456789012345678901234567890123456789","changedFiles":2,"isDraft":false,"reviewDecision":"APPROVED","latestOpinion":{"nodes":[{"state":state}]},"recentReviews":{"nodes":[{"state":"APPROVED","author":{"login":"reviewer"}},{"state":"CHANGES_REQUESTED","author":{"login":"reviewer"}}],"pageInfo":{"hasPreviousPage":false}},"reviewThreads":{"totalCount":2}})
    }

    struct SearchFixture {
        calls: Mutex<Vec<ApiRequest>>,
        repeat_cursor: bool,
        personal: bool,
    }

    #[test]
    fn lightweight_lists_do_not_invent_review_counts_or_loaded_descriptions() {
        let mut value = node(7, "APPROVED");
        value.as_object_mut().unwrap().remove("body");
        value.as_object_mut().unwrap().remove("recentReviews");
        let summary = PullSummary::from_graphql(&value).unwrap();
        assert!(summary.i_approved);
        assert_eq!(summary.approval_count, None);
        assert_eq!(summary.body, None);
        let metadata = PullSummary::from_graphql(&node(7, "APPROVED")).unwrap();
        assert_eq!(
            metadata.approval_count,
            Some(0),
            "a later changes-requested review replaces the old approval"
        );
        assert_eq!(metadata.body.as_deref(), Some("The PR body"));
    }

    impl Transport for SearchFixture {
        fn request<'a>(
            &'a self,
            _: &'a AccountScope,
            request: ApiRequest,
        ) -> super::super::transport::ApiFuture<'a> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(request.clone());
                if request.method == reqwest::Method::GET {
                    return Ok(ApiResponse {
                        status: 200,
                        headers: Default::default(),
                        body: json!({"type":if self.personal {"User"}else{"Organization"}}),
                    });
                }
                let variables = &request.body.as_ref().unwrap()["variables"];
                let next = !variables["after"].is_null();
                let body = json!({"data":{"search":{"issueCount":2,"nodes":if next {vec![node(7,"APPROVED"),node(8,"COMMENTED")]}else{vec![node(7,"APPROVED")]},"pageInfo":{"hasNextPage":!next||self.repeat_cursor,"endCursor":"page-2"}}}});
                Ok(ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body,
                })
            })
        }
    }

    #[tokio::test]
    async fn modes_page_merge_and_classify_actual_latest_review_without_duplicate_cards() {
        let transport = Arc::new(SearchFixture {
            calls: Mutex::new(Vec::new()),
            repeat_cursor: false,
            personal: false,
        });
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("HemSoft", "team").unwrap());
        let needs = client
            .pull_requests(PrMode::NeedsReview, 7, true)
            .await
            .unwrap();
        assert_eq!(needs.items.len(), 1);
        assert_eq!(needs.items[0].target.number, 8);
        assert_eq!(needs.items[0].approval_count, Some(0));
        assert_eq!(
            transport.calls.lock().unwrap().len(),
            7,
            "namespace lookup plus all three incumbent searches with both pages"
        );
        let nudge = client
            .pull_requests(PrMode::NeedANudge, 7, true)
            .await
            .unwrap();
        assert_eq!(
            nudge
                .items
                .iter()
                .map(|p| p.target.number)
                .collect::<Vec<_>>(),
            [7]
        );
        assert!(nudge.items[0].matches_filter("feature/fixture"));
        assert!(!nudge.items[0].matches_filter("unrelated"));
        assert!(
            transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == reqwest::Method::POST)
                .all(|r| r.body.as_ref().unwrap()["variables"]["reviewer"] == "HemSoft")
        );
    }

    #[tokio::test]
    async fn repeated_pagination_cursor_is_a_visible_failure_not_an_infinite_loop() {
        let transport = Arc::new(SearchFixture {
            calls: Mutex::new(Vec::new()),
            repeat_cursor: true,
            personal: false,
        });
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("first", "team").unwrap());
        assert!(matches!(
            client.pull_requests(PrMode::MyPrs, 7, false).await,
            Err(GitHubError::InvalidData)
        ));
        assert_eq!(transport.calls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn personal_namespaces_use_user_scope_instead_of_an_organization_search() {
        let transport = Arc::new(SearchFixture {
            calls: Mutex::new(Vec::new()),
            repeat_cursor: false,
            personal: true,
        });
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("First", "Personal").unwrap());
        client.pull_requests(PrMode::MyPrs, 7, true).await.unwrap();
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls[0].path, "/users/Personal");
        let query = calls[1].body.as_ref().unwrap()["variables"]["query"]
            .as_str()
            .unwrap();
        assert!(query.contains("user:Personal"));
        assert!(!query.contains("org:Personal"));
    }
}
