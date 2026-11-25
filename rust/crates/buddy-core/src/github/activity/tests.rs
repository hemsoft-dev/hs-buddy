use super::*;
use crate::github::{AccountScope, ApiResponse, GitHubClients, Transport};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn summary(name: &str, archived: bool) -> RepositorySummary {
    RepositorySummary::parse(&json!({
        "full_name": format!("Fixture-User/{name}"),
        "private": false, "archived": archived, "fork": false,
        "stargazers_count": 0, "forks_count": 0, "open_issues_count": 0, "size": 1,
        "updated_at": "2026-10-01T00:00:00Z"
    }))
    .unwrap()
}

fn found(repo: &str, number: u64, state: &str, updated: &str) -> Value {
    json!({
        "repository_url": format!("https://api.github.com/repos/fixture-user/{repo}"),
        "number": number, "title": format!("Item {number}"), "state": state,
        "updated_at": updated, "html_url": "https://attacker.invalid/ignored"
    })
}

fn lane(total: u64, items: Vec<Value>, kind: Kind) -> Result<Lane, GitHubError> {
    Lane::parse(&json!({"total_count": total, "items": items}), kind)
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

#[test]
fn activity_groups_known_live_repositories_and_ranks_them_like_electron() {
    let repositories = [
        summary("Alpha", false),
        summary("beta", false),
        summary("Retired", true),
    ];
    let mut merged = found("beta", 30, "closed", "2026-10-14T00:00:00Z");
    merged["pull_request"] = json!({"merged_at": "2026-10-14T00:00:00Z"});
    let mut draft = found("beta", 31, "open", "2026-10-10T00:00:00Z");
    draft["draft"] = json!(true);
    let issues = lane(
        6,
        vec![
            found("alpha", 1, "closed", "2026-10-15T00:00:00Z"),
            found("alpha", 2, "open", "2026-10-11T00:00:00Z"),
            found("alpha", 3, "open", "2026-10-12T00:00:00Z"),
            found("alpha", 4, "open", "2026-10-09T00:00:00Z"),
            found("alpha", 5, "open", "2026-10-08T00:00:00Z"),
            // Archived and unlisted repositories are not on the workbench.
            found("retired", 6, "open", "2026-10-16T00:00:00Z"),
            found("elsewhere", 7, "open", "2026-10-16T00:00:00Z"),
        ],
        Kind::Issue,
    );
    let pulls = lane(2, vec![merged, draft], Kind::PullRequest);
    let activity = build(&repositories, issues, pulls, at("2026-10-16T00:00:00Z"));

    assert!(!activity.partial() && !activity.unavailable());
    let names: Vec<_> = activity.repositories.iter().map(|r| r.name()).collect();
    assert_eq!(names, ["Alpha", "beta"]);
    let alpha = &activity.repositories[0];
    assert_eq!(alpha.full_name, "Fixture-User/Alpha");
    assert_eq!(alpha.updated_at, at("2026-10-15T00:00:00Z"));
    // Open items first, newest first, four per lane; the closed #1 drops off.
    let numbers: Vec<_> = alpha.issues.iter().map(|item| item.number).collect();
    assert_eq!(numbers, [3, 2, 4, 5]);
    assert_eq!(
        alpha.issues[0].url,
        "https://github.com/fixture-user/alpha/issues/3"
    );
    let beta = &activity.repositories[1];
    let states: Vec<_> = beta.pull_requests.iter().map(|item| item.state).collect();
    assert_eq!(states, [ActivityState::Draft, ActivityState::Merged]);
    assert_eq!(
        beta.pull_requests[1].url,
        "https://github.com/fixture-user/beta/pull/30"
    );
    assert_eq!(ActivityState::Merged.label(), "merged");
    // Nine items returned of eight counted: nothing is beyond this sample.
    assert!(!activity.has_more);
}

#[test]
fn more_activity_is_offered_beyond_six_repositories_or_the_search_sample() {
    let repositories: Vec<_> = (0..7).map(|n| summary(&format!("r{n}"), false)).collect();
    let items = (0..7)
        .map(|n| {
            found(
                &format!("r{n}"),
                n + 1,
                "open",
                &format!("2026-10-1{n}T00:00:00Z"),
            )
        })
        .collect();
    let activity = build(
        &repositories,
        lane(7, items, Kind::Issue),
        lane(0, vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    let names: Vec<_> = activity.repositories.iter().map(|r| r.name()).collect();
    assert_eq!(names, ["r6", "r5", "r4", "r3", "r2", "r1"]);
    assert!(activity.has_more);

    let sampled = build(
        &repositories,
        lane(
            120,
            vec![found("r0", 1, "open", "2026-10-10T00:00:00Z")],
            Kind::Issue,
        ),
        lane(0, vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    assert_eq!(sampled.repositories.len(), 1);
    assert!(sampled.has_more);
}

#[test]
fn a_failed_search_leaves_its_lane_unknown_rather_than_empty() {
    let repositories = [summary("alpha", false)];
    let partial = build(
        &repositories,
        lane(
            1,
            vec![found("alpha", 1, "open", "2026-10-10T00:00:00Z")],
            Kind::Issue,
        ),
        Err(GitHubError::RateLimited {
            retry_after_seconds: 60,
        }),
        Timestamp::UNIX_EPOCH,
    );
    assert!(partial.partial() && !partial.unavailable());
    assert!(partial.issues_error.is_none());
    assert!(matches!(
        partial.pull_requests_error,
        Some(GitHubError::RateLimited { .. })
    ));
    assert_eq!(partial.repositories.len(), 1);

    let none = build(
        &repositories,
        Err(GitHubError::Network),
        Err(GitHubError::Network),
        Timestamp::UNIX_EPOCH,
    );
    assert!(none.unavailable() && !none.partial());
    assert!(none.repositories.is_empty() && !none.has_more);

    // One malformed item makes the whole lane unknown, never a short list.
    for broken in [
        json!({"repository_url": "https://api.github.com/users/x", "number": 1}),
        found("alpha", 0, "open", "2026-10-10T00:00:00Z"),
        found("alpha", 1, "reopened", "2026-10-10T00:00:00Z"),
        found("alpha", 1, "open", "yesterday"),
    ] {
        assert_eq!(
            lane(1, vec![broken], Kind::Issue).err(),
            Some(GitHubError::InvalidData)
        );
    }
    assert_eq!(
        Lane::parse(&json!({"items": []}), Kind::Issue).err(),
        Some(GitHubError::InvalidData)
    );
}

fn timed_out(items: Vec<Value>, kind: Kind) -> Result<Lane, GitHubError> {
    Lane::parse(
        &json!({"total_count": items.len(), "incomplete_results": true, "items": items}),
        kind,
    )
}

#[test]
fn a_timed_out_search_is_never_read_as_a_complete_empty_lane() {
    let repositories = [summary("alpha", false)];
    // Nothing to show from a timed-out search: the lane is unread, like a failure.
    let empty = build(
        &repositories,
        timed_out(vec![], Kind::Issue),
        lane(0, vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    assert_eq!(empty.issues_error, Some(GitHubError::IncompleteSearch));
    assert!(empty.partial() && empty.uncertain());
    assert!(!empty.issues_incomplete);

    // Items only for unlisted repositories still leave nothing to show.
    let elsewhere = build(
        &repositories,
        timed_out(
            vec![found("elsewhere", 1, "open", "2026-10-10T00:00:00Z")],
            Kind::Issue,
        ),
        timed_out(vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    assert!(elsewhere.unavailable());
    assert!(elsewhere.repositories.is_empty());

    // Items from a timed-out search are kept and the lane may be missing some.
    let some = build(
        &repositories,
        timed_out(
            vec![found("alpha", 2, "open", "2026-10-10T00:00:00Z")],
            Kind::Issue,
        ),
        lane(0, vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    assert!(some.issues_error.is_none() && some.issues_incomplete);
    assert!(!some.pull_requests_incomplete && some.uncertain());
    assert_eq!(some.repositories[0].issues[0].number, 2);
    assert!(some.has_more, "GitHub may have more than it returned");

    // Complete searches are certain; a malformed flag is unknown, not false.
    let complete = build(
        &repositories,
        lane(0, vec![], Kind::Issue),
        lane(0, vec![], Kind::PullRequest),
        Timestamp::UNIX_EPOCH,
    );
    assert!(!complete.uncertain() && complete.repositories.is_empty());
    assert_eq!(
        Lane::parse(
            &json!({"total_count": 0, "incomplete_results": "yes", "items": []}),
            Kind::Issue
        )
        .err(),
        Some(GitHubError::InvalidData)
    );
}

#[test]
fn all_activity_links_to_the_namespace_search_like_electron() {
    assert_eq!(
        all_activity_url("Fixture-User"),
        "https://github.com/search?q=user%3AFixture-User%20sort%3Aupdated-desc&type=issues"
    );
}

#[derive(Default)]
struct SearchFixture(Mutex<Vec<String>>);

impl Transport for SearchFixture {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        request: ApiRequest,
    ) -> crate::github::transport::ApiFuture<'a> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request.path.clone());
            let (status, body) = if request.path.contains("is%3Apr") {
                (403, Value::Null)
            } else {
                (
                    200,
                    json!({"total_count": 1, "items": [found("alpha", 9, "open", "2026-10-10T00:00:00Z")]}),
                )
            };
            Ok(ApiResponse {
                status,
                headers: BTreeMap::from([("x-ratelimit-remaining".into(), "0".into())]),
                body,
            })
        })
    }
}

#[tokio::test]
async fn the_workbench_reads_two_namespace_searches_and_keeps_the_one_that_answered() {
    let transport = Arc::new(SearchFixture::default());
    let client = GitHubClients::with_transport(transport.clone())
        .for_account(AccountScope::new("fixture-user", "Fixture-User").unwrap());
    let activity = client
        .repository_activity(&[summary("alpha", false)], false)
        .await;
    let mut paths = transport.0.lock().unwrap().clone();
    paths.sort();
    assert_eq!(
        paths,
        [
            "/search/issues?q=user%3AFixture-User+is%3Aissue&sort=updated&order=desc&per_page=50",
            "/search/issues?q=user%3AFixture-User+is%3Apr&sort=updated&order=desc&per_page=50",
        ]
    );
    assert_eq!(activity.repositories[0].issues[0].number, 9);
    assert!(matches!(
        activity.pull_requests_error,
        Some(GitHubError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn activity_reused_from_the_cache_keeps_the_time_github_returned_it() {
    let transport = Arc::new(SearchFixture::default());
    let client = GitHubClients::with_transport(transport.clone())
        .for_account(AccountScope::new("fixture-user", "Fixture-User").unwrap());
    let repositories = [summary("alpha", false)];
    let first = client.repository_activity(&repositories, false).await;
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Reopening inside the cache's lifetime reuses the issue search; the
    // limited pull-request search is read again.
    let reused = client.repository_activity(&repositories, false).await;
    assert_eq!(transport.0.lock().unwrap().len(), 3);
    assert_eq!(reused.repositories[0].issues[0].number, 9);
    let drift = reused.fetched_at.duration_since(first.fetched_at);
    assert!(
        drift.abs() < jiff::SignedDuration::from_millis(100),
        "{drift:?}"
    );
    assert!(
        Timestamp::now().duration_since(reused.fetched_at)
            >= jiff::SignedDuration::from_millis(300)
    );

    // A forced refresh reads both searches now.
    let refreshed = client.repository_activity(&repositories, true).await;
    assert!(
        Timestamp::now().duration_since(refreshed.fetched_at)
            < jiff::SignedDuration::from_millis(100)
    );
}

#[test]
fn the_read_time_is_that_of_the_oldest_shown_result() {
    let now = at("2026-10-15T12:00:00Z");
    assert_eq!(read_time(now, []), now);
    assert_eq!(
        read_time(
            now,
            [
                std::time::Duration::ZERO,
                std::time::Duration::from_secs(42)
            ]
        ),
        at("2026-10-15T11:59:18Z")
    );
}
