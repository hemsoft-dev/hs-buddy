//! The organization overview's "Current workbench": recent issue and pull
//! request activity grouped by repository, as Electron's
//! `repositoryActivity.ts` builds it for a personal namespace.
use std::collections::HashMap;

use jiff::Timestamp;
use serde_json::Value;

use super::pulls::text;
use super::{ApiRequest, GitHubClient, GitHubError, PullRef, RepositoryRef, RepositorySummary};

/// Electron's `ACTIVE_REPOSITORY_LIMIT` and `ACTIVITY_ITEMS_PER_REPOSITORY`.
const ACTIVE_REPOSITORY_LIMIT: usize = 6;
const ACTIVITY_ITEMS_PER_REPOSITORY: usize = 4;
/// Each search reads one page of this many recently updated items.
const SEARCH_ITEMS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    Open,
    Draft,
    Closed,
    Merged,
}

impl ActivityState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Draft)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: ActivityState,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRepository {
    pub repository: RepositoryRef,
    /// `owner/name` as GitHub spells it; the reference is lowercase.
    pub full_name: String,
    /// The newest update among its issues and pull requests.
    pub updated_at: Timestamp,
    pub issues: Vec<ActivityItem>,
    pub pull_requests: Vec<ActivityItem>,
}

#[derive(Debug, Clone)]
pub struct RepositoryActivity {
    pub repositories: Vec<ActiveRepository>,
    /// Why a lane could not be read; its items are then unknown, not empty.
    /// A search that timed out with nothing to show is unread as well.
    pub issues_error: Option<GitHubError>,
    pub pull_requests_error: Option<GitHubError>,
    /// GitHub's search timed out after returning these items, so the lane
    /// may be missing some.
    pub issues_incomplete: bool,
    pub pull_requests_incomplete: bool,
    pub has_more: bool,
    pub fetched_at: Timestamp,
}

impl ActiveRepository {
    /// The repository's name without its owner, as Electron's card shows it.
    pub fn name(&self) -> &str {
        self.full_name
            .split_once('/')
            .map_or(self.full_name.as_str(), |(_, name)| name)
    }
}

impl RepositoryActivity {
    /// Neither search returned, so there is no activity to rank.
    pub fn unavailable(&self) -> bool {
        self.issues_error.is_some() && self.pull_requests_error.is_some()
    }

    pub fn partial(&self) -> bool {
        self.issues_error.is_some() != self.pull_requests_error.is_some()
    }

    /// Either lane is unread or may be missing items, so an empty list does
    /// not mean there was no activity.
    pub fn uncertain(&self) -> bool {
        self.issues_error.is_some()
            || self.pull_requests_error.is_some()
            || self.issues_incomplete
            || self.pull_requests_incomplete
    }
}

/// Electron's "All activity" link: GitHub's search of the namespace.
pub fn all_activity_url(owner: &str) -> String {
    let query: String =
        url::form_urlencoded::byte_serialize(format!("user:{owner} sort:updated-desc").as_bytes())
            .collect();
    // `encodeURIComponent` spells a space %20, not `+`.
    format!(
        "https://github.com/search?q={}&type=issues",
        query.replace('+', "%20")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Issue,
    PullRequest,
}

#[derive(Debug, Clone)]
struct Lane {
    total: u64,
    /// GitHub's `incomplete_results`: the search timed out before it finished.
    incomplete: bool,
    items: Vec<(RepositoryRef, ActivityItem)>,
}

impl Lane {
    fn parse(body: &Value, kind: Kind) -> Result<Self, GitHubError> {
        let total = body["total_count"]
            .as_u64()
            .ok_or(GitHubError::InvalidData)?;
        let incomplete = match body.get("incomplete_results") {
            None => false,
            Some(flag) => flag.as_bool().ok_or(GitHubError::InvalidData)?,
        };
        let items = body["items"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?
            .iter()
            .map(|value| item(value, kind))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            total,
            incomplete,
            items,
        })
    }
}

fn item(value: &Value, kind: Kind) -> Result<(RepositoryRef, ActivityItem), GitHubError> {
    let api = text(value, "repository_url")?;
    let (_, full_name) = api.split_once("/repos/").ok_or(GitHubError::InvalidData)?;
    let repository = RepositoryRef::parse(full_name)?;
    let number = value["number"].as_u64().ok_or(GitHubError::InvalidData)?;
    let pull = PullRef::new(repository.clone(), number).map_err(|_| GitHubError::InvalidData)?;
    let open = match text(value, "state")? {
        "open" => true,
        "closed" => false,
        _ => return Err(GitHubError::InvalidData),
    };
    let state = match kind {
        Kind::PullRequest if value["pull_request"]["merged_at"].is_string() => {
            ActivityState::Merged
        }
        Kind::PullRequest if open && value["draft"].as_bool() == Some(true) => ActivityState::Draft,
        _ if open => ActivityState::Open,
        _ => ActivityState::Closed,
    };
    let url = match kind {
        Kind::Issue => format!("{}/issues/{number}", repository.web_url()),
        Kind::PullRequest => pull.web_url(),
    };
    let updated_at = text(value, "updated_at")?
        .parse::<Timestamp>()
        .map_err(|_| GitHubError::InvalidData)?;
    Ok((
        repository,
        ActivityItem {
            number,
            title: text(value, "title")?.into(),
            url,
            state,
            updated_at,
        },
    ))
}

/// Electron's `compareActivityItems`: open and draft first, then newest.
fn compare_items(left: &ActivityItem, right: &ActivityItem) -> std::cmp::Ordering {
    right
        .state
        .is_open()
        .cmp(&left.state.is_open())
        .then(right.updated_at.cmp(&left.updated_at))
        .then(right.number.cmp(&left.number))
}

/// Electron's `buildRepositoryActivity`: the namespace's non-archived
/// repositories that appear in either search, newest first.
fn build(
    repositories: &[RepositorySummary],
    issues: Result<Lane, GitHubError>,
    pull_requests: Result<Lane, GitHubError>,
    fetched_at: Timestamp,
) -> RepositoryActivity {
    let known: HashMap<String, &RepositorySummary> = repositories
        .iter()
        .filter(|repository| !repository.archived)
        .map(|repository| (repository.target.full_name(), repository))
        .collect();
    let mut grouped: HashMap<String, ActiveRepository> = HashMap::new();
    let mut returned = 0;
    let mut total = 0;
    for (lane, kind) in [(&issues, Kind::Issue), (&pull_requests, Kind::PullRequest)] {
        let Ok(lane) = lane else { continue };
        returned += lane.items.len() as u64;
        total += lane.total;
        for (repository, item) in &lane.items {
            let key = repository.full_name();
            let Some(summary) = known.get(&key) else {
                continue;
            };
            let entry = grouped.entry(key).or_insert_with(|| ActiveRepository {
                repository: summary.target.clone(),
                full_name: summary.full_name.clone(),
                updated_at: item.updated_at,
                issues: Vec::new(),
                pull_requests: Vec::new(),
            });
            entry.updated_at = entry.updated_at.max(item.updated_at);
            match kind {
                Kind::Issue => entry.issues.push(item.clone()),
                Kind::PullRequest => entry.pull_requests.push(item.clone()),
            }
        }
    }
    let mut ranked: Vec<ActiveRepository> = grouped
        .into_values()
        .map(|mut repository| {
            for lane in [&mut repository.issues, &mut repository.pull_requests] {
                lane.sort_by(compare_items);
                lane.truncate(ACTIVITY_ITEMS_PER_REPOSITORY);
            }
            repository
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.repository.cmp(&right.repository))
    });
    let has_more = ranked.len() > ACTIVE_REPOSITORY_LIMIT || total > returned;
    ranked.truncate(ACTIVE_REPOSITORY_LIMIT);
    // A timed-out search that left nothing to show is unread, not empty.
    let settle = |lane: Result<Lane, GitHubError>, shown: usize| match lane {
        Ok(lane) if lane.incomplete && shown == 0 => (Err(GitHubError::IncompleteSearch), false),
        Ok(lane) => (Ok(()), lane.incomplete),
        Err(error) => (Err(error), false),
    };
    let shown = |lane: fn(&ActiveRepository) -> &Vec<ActivityItem>| {
        ranked.iter().map(|repository| lane(repository).len()).sum()
    };
    let (issues, issues_incomplete) = settle(issues, shown(|r| &r.issues));
    let (pull_requests, pull_requests_incomplete) =
        settle(pull_requests, shown(|r| &r.pull_requests));
    let has_more = has_more || issues_incomplete || pull_requests_incomplete;
    RepositoryActivity {
        repositories: ranked,
        issues_error: issues.err(),
        pull_requests_error: pull_requests.err(),
        issues_incomplete,
        pull_requests_incomplete,
        has_more,
        fetched_at,
    }
}

/// When GitHub returned the oldest of the shown results. A search reused from
/// the response cache keeps the time it was read, so the header never calls a
/// cached result fresh.
fn read_time(now: Timestamp, ages: impl IntoIterator<Item = std::time::Duration>) -> Timestamp {
    ages.into_iter()
        .max()
        .and_then(|age| jiff::SignedDuration::try_from(age).ok())
        .and_then(|age| now.checked_sub(age).ok())
        .unwrap_or(now)
}

impl GitHubClient {
    /// Two fixed searches of the selected namespace; a failed one leaves its
    /// lane unknown while the other still ranks repositories.
    pub async fn repository_activity(
        &self,
        repositories: &[RepositorySummary],
        force: bool,
    ) -> RepositoryActivity {
        let search = |kind: Kind| {
            let qualifier = match kind {
                Kind::Issue => "is:issue",
                Kind::PullRequest => "is:pr",
            };
            let query: String = url::form_urlencoded::byte_serialize(
                format!("user:{} {qualifier}", self.account.organization()).as_bytes(),
            )
            .collect();
            async move {
                let loaded = self
                    .request(
                        ApiRequest::get(format!(
                            "/search/issues?q={query}&sort=updated&order=desc&per_page={SEARCH_ITEMS}"
                        )),
                        force,
                    )
                    .await?;
                Ok::<_, GitHubError>((Lane::parse(&loaded.value.body, kind)?, loaded.age))
            }
        };
        let (issues, pull_requests) =
            futures::join!(search(Kind::Issue), search(Kind::PullRequest));
        let fetched_at = read_time(
            Timestamp::now(),
            [&issues, &pull_requests]
                .into_iter()
                .filter_map(|lane| lane.as_ref().ok().map(|(_, age)| *age)),
        );
        build(
            repositories,
            issues.map(|(lane, _)| lane),
            pull_requests.map(|(lane, _)| lane),
            fetched_at,
        )
    }
}

#[cfg(test)]
mod tests;
