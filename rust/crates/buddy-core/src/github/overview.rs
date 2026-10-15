use std::collections::BTreeMap;

use super::browsing::number;
use super::details::web_url;
use super::pulls::text;
use super::{
    ApiRequest, Commit, GitHubClient, GitHubError, RepositoryRef, RepositorySummary, UserSummary,
};

#[derive(Debug, Clone)]
pub struct Contributor {
    pub user: UserSummary,
    pub contributions: u64,
}

#[derive(Debug, Clone)]
pub struct WorkflowSummary {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepositoryDetail {
    pub repository: RepositorySummary,
    pub open_issues: Option<u64>,
    pub open_prs: Option<u64>,
    pub languages: BTreeMap<String, u64>,
    pub contributors: Vec<Contributor>,
    pub recent_commits: Vec<Commit>,
    pub latest_workflow: Option<WorkflowSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OrganizationOverview {
    pub repositories: Vec<RepositorySummary>,
    pub private_repositories: u64,
    pub archived_repositories: u64,
    pub stars: u64,
    pub forks: u64,
    pub open_items: u64,
    pub last_push: Option<String>,
    pub partial: bool,
}

impl GitHubClient {
    pub async fn repository_detail(
        &self,
        repository: &RepositoryRef,
        force: bool,
    ) -> Result<RepositoryDetail, GitHubError> {
        let summary = self.repository(repository, force).await?;
        let path = repository.api_path();
        let (counts, languages, contributors, commits, workflow) = futures::join!(
            self.repository_counts(repository, force),
            self.request(ApiRequest::get(format!("{path}/languages")), force),
            self.request(
                ApiRequest::get(format!("{path}/contributors?per_page=10")),
                force
            ),
            self.request(ApiRequest::get(format!("{path}/commits?per_page=5")), force),
            self.request(
                ApiRequest::get(format!("{path}/actions/runs?per_page=1")),
                force
            )
        );
        let mut detail = RepositoryDetail {
            repository: summary,
            open_issues: None,
            open_prs: None,
            languages: BTreeMap::new(),
            contributors: Vec::new(),
            recent_commits: Vec::new(),
            latest_workflow: None,
            warnings: Vec::new(),
        };
        match counts {
            Ok((issues, prs)) => {
                detail.open_issues = Some(issues);
                detail.open_prs = Some(prs);
            }
            Err(error) => detail.warnings.push(format!("Issue/PR counts: {error}")),
        }
        let parsed_languages = languages.and_then(|loaded| {
            loaded
                .value
                .body
                .as_object()
                .ok_or(GitHubError::InvalidData)?
                .iter()
                .map(|(name, value)| {
                    value
                        .as_u64()
                        .map(|count| (name.clone(), count))
                        .ok_or(GitHubError::InvalidData)
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        });
        match parsed_languages {
            Ok(languages) => detail.languages = languages,
            Err(error) => detail.warnings.push(format!("Languages: {error}")),
        }
        let parsed_contributors = contributors.and_then(|loaded| {
            loaded
                .value
                .body
                .as_array()
                .ok_or(GitHubError::InvalidData)?
                .iter()
                .map(|value| {
                    Ok(Contributor {
                        user: UserSummary::parse(value)?,
                        contributions: number(value, "contributions")?,
                    })
                })
                .collect::<Result<Vec<_>, GitHubError>>()
        });
        match parsed_contributors {
            Ok(contributors) => detail.contributors = contributors,
            Err(error) => detail.warnings.push(format!("Contributors: {error}")),
        }
        let parsed_commits = commits.and_then(|loaded| {
            loaded
                .value
                .body
                .as_array()
                .ok_or(GitHubError::InvalidData)?
                .iter()
                .map(|value| Commit::from_rest(repository, value))
                .collect::<Result<Vec<_>, _>>()
        });
        match parsed_commits {
            Ok(commits) => detail.recent_commits = commits,
            Err(GitHubError::EmptyRepository) => {}
            Err(error) => detail.warnings.push(format!("Recent commits: {error}")),
        }
        let parsed_workflow = workflow.and_then(|loaded| {
            let values = loaded.value.body["workflow_runs"]
                .as_array()
                .ok_or(GitHubError::InvalidData)?;
            values
                .first()
                .map(|value| {
                    Ok(WorkflowSummary {
                        name: text(value, "name")?.into(),
                        status: text(value, "status")?.into(),
                        conclusion: value["conclusion"].as_str().map(Into::into),
                        branch: value["head_branch"].as_str().map(Into::into),
                        created_at: text(value, "created_at")?.into(),
                        url: web_url(&value["html_url"]),
                    })
                })
                .transpose()
        });
        match parsed_workflow {
            Ok(workflow) => detail.latest_workflow = workflow,
            Err(error) => detail.warnings.push(format!("Latest workflow: {error}")),
        }
        Ok(detail)
    }

    pub async fn organization_overview(
        &self,
        force: bool,
    ) -> Result<OrganizationOverview, GitHubError> {
        let mut repositories = Vec::new();
        let mut cursor = None;
        let mut partial = false;
        loop {
            let page = self.repositories(cursor.as_ref(), force).await?;
            repositories.extend(page.items);
            partial |= page.truncated;
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let private_repositories = repositories
            .iter()
            .filter(|r| r.visibility != "public")
            .count() as u64;
        let archived_repositories = repositories.iter().filter(|r| r.archived).count() as u64;
        let stars = repositories.iter().map(|r| r.stars).sum();
        let forks = repositories.iter().map(|r| r.forks).sum();
        let open_items = repositories.iter().map(|r| r.open_items).sum();
        let last_push = repositories
            .iter()
            .filter_map(|r| r.pushed_at.as_ref())
            .max()
            .cloned();
        Ok(OrganizationOverview {
            repositories,
            private_repositories,
            archived_repositories,
            stars,
            forks,
            open_items,
            last_push,
            partial,
        })
    }
}
