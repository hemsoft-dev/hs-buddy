use std::collections::BTreeMap;

use super::details::{actor, valid_sha, web_url};
use super::pulls::text;
use super::{GitHubClient, GitHubError, Page, PageCursor, PullRef, RepositoryRef};

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub state: String,
    pub conclusion: Option<String>,
    pub app: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckRollup {
    pub head_sha: String,
    pub checks: Vec<Check>,
    pub partial: bool,
}

impl CheckRollup {
    pub fn state(&self) -> &'static str {
        if self.checks.iter().any(|check| {
            matches!(
                check.conclusion.as_deref(),
                Some(
                    "failure"
                        | "timed_out"
                        | "cancelled"
                        | "action_required"
                        | "startup_failure"
                        | "error"
                )
            )
        }) {
            return "failing";
        }
        if self.partial {
            return "partial";
        }
        if self.checks.is_empty() {
            return "none";
        }
        if self.checks.iter().any(|check| {
            check.state != "completed"
                || !matches!(
                    check.conclusion.as_deref(),
                    Some("success" | "neutral" | "skipped")
                )
        }) {
            return "pending";
        }
        if self
            .checks
            .iter()
            .all(|check| matches!(check.conclusion.as_deref(), Some("neutral" | "skipped")))
        {
            return "neutral";
        }
        "passing"
    }
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub kind: String,
    pub author: String,
    pub date: String,
    pub body: String,
    pub url: Option<String>,
}

impl GitHubClient {
    /// Query one immutable head for checks and statuses. Results from another
    /// head are never combined into this rollup.
    pub async fn pull_checks(
        &self,
        pull: &PullRef,
        head_sha: &str,
        force: bool,
    ) -> Result<CheckRollup, GitHubError> {
        self.pull_variables(pull)?;
        valid_sha(head_sha)?;
        let root = format!("{}/commits/{head_sha}", pull.repository.api_path());
        let mut latest = BTreeMap::new();
        let mut partial = false;
        let mut cursor = None;
        loop {
            let page = self
                .rest_page(
                    &format!("{root}/check-runs?filter=latest"),
                    cursor.as_ref(),
                    Some("check_runs"),
                    force,
                )
                .await?;
            partial |= page.truncated;
            for value in page.items {
                let id = value["id"].as_u64().ok_or(GitHubError::InvalidData)?;
                let name = text(&value, "name")?.to_string();
                let app = value["app"]["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| format!("unknown-app-{id}"));
                let key = (app, name.clone());
                if latest
                    .get(&key)
                    .is_some_and(|(previous, _): &(u64, Check)| *previous >= id)
                {
                    continue;
                }
                latest.insert(
                    key,
                    (
                        id,
                        Check {
                            name: text(&value, "name")?.into(),
                            state: text(&value, "status")?.into(),
                            conclusion: value["conclusion"].as_str().map(Into::into),
                            app: value["app"]["name"].as_str().map(Into::into),
                            started_at: value["started_at"].as_str().map(Into::into),
                            completed_at: value["completed_at"].as_str().map(Into::into),
                            url: web_url(&value["details_url"]),
                        },
                    ),
                );
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let mut statuses = BTreeMap::new();
        cursor = None;
        loop {
            let page = self
                .rest_page(&format!("{root}/statuses"), cursor.as_ref(), None, force)
                .await?;
            partial |= page.truncated;
            for value in page.items {
                let name = text(&value, "context")?.to_string();
                // REST statuses are returned newest first. Retain the first
                // occurrence across all pages, not an older later-page state.
                if statuses.contains_key(&name) {
                    continue;
                }
                let state = text(&value, "state")?;
                statuses.insert(
                    name.clone(),
                    Check {
                        name,
                        state: if state == "pending" {
                            "in_progress"
                        } else {
                            "completed"
                        }
                        .into(),
                        conclusion: Some(state.into()),
                        app: None,
                        started_at: value["created_at"].as_str().map(Into::into),
                        completed_at: value["updated_at"].as_str().map(Into::into),
                        url: web_url(&value["target_url"]),
                    },
                );
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let mut checks = latest
            .into_values()
            .map(|(_, check)| check)
            .collect::<Vec<_>>();
        checks.extend(statuses.into_values());
        checks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(CheckRollup {
            head_sha: head_sha.into(),
            checks,
            partial,
        })
    }

    pub async fn pull_history(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<HistoryEntry>, GitHubError> {
        self.pull_variables(pull)?;
        self.rest_page(
            &format!(
                "{}/issues/{}/timeline",
                pull.repository.api_path(),
                pull.number
            ),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| {
            let kind = text(&value, "event")?.to_string();
            let author = value["actor"]["login"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| actor(&value));
            let date = value["created_at"]
                .as_str()
                .or_else(|| value["committer"]["date"].as_str())
                .or_else(|| value["submitted_at"].as_str())
                .unwrap_or("")
                .to_string();
            let body = value["body"]
                .as_str()
                .or_else(|| value["message"].as_str())
                .or_else(|| value["state"].as_str())
                .unwrap_or("")
                .to_string();
            Ok(HistoryEntry {
                kind,
                author,
                date,
                body,
                url: web_url(&value["html_url"]),
            })
        })
    }

    pub async fn repository_has_coderabbit(
        &self,
        repository: &RepositoryRef,
        force: bool,
    ) -> Result<bool, GitHubError> {
        match self
            .request(
                super::ApiRequest::get(format!(
                    "{}/contents/.coderabbit.yaml",
                    repository.api_path()
                )),
                force,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(GitHubError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AccountScope, ApiRequest, ApiResponse, GitHubClients, Transport};
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CheckFixture {
        calls: Mutex<Vec<String>>,
    }

    impl Transport for CheckFixture {
        fn request<'a>(
            &'a self,
            _: &'a AccountScope,
            request: ApiRequest,
        ) -> super::super::transport::ApiFuture<'a> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(request.path.clone());
                let body = if request.path.contains("check-runs") {
                    json!({"check_runs":[
                     {"id":200,"name":"ci-complete","status":"completed","conclusion":"success","app":{"id":1,"name":"GitHub Actions"},"details_url":"https://github.com/team/repo/actions/runs/2"},
                     {"id":100,"name":"ci-complete","status":"completed","conclusion":"failure","app":{"id":1,"name":"GitHub Actions"},"details_url":"https://github.com/team/repo/actions/runs/1"}
                    ]})
                } else {
                    json!([])
                };
                Ok(ApiResponse {
                    status: 200,
                    headers: Default::default(),
                    body,
                })
            })
        }
    }

    #[tokio::test]
    async fn a_superseded_failure_does_not_override_the_latest_check_for_the_same_app() {
        let transport = Arc::new(CheckFixture::default());
        let client = GitHubClients::with_transport(transport.clone())
            .for_account(AccountScope::new("first", "team").unwrap());
        let pull = PullRef::new(RepositoryRef::parse("team/repo").unwrap(), 7).unwrap();
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let checks = client.pull_checks(&pull, sha, true).await.unwrap();
        assert_eq!(checks.checks.len(), 1);
        assert_eq!(checks.state(), "passing");
        assert_eq!(
            checks.checks[0].url.as_deref(),
            Some("https://github.com/team/repo/actions/runs/2")
        );
        assert!(
            transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|path| path.contains(sha))
        );
    }
}
