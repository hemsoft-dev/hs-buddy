use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

struct Fixture {
    responses: Mutex<VecDeque<ApiResponse>>,
    calls: Mutex<Vec<(AccountScope, ApiRequest)>>,
}
impl Transport for Fixture {
    fn request<'a>(
        &'a self,
        account: &'a AccountScope,
        request: ApiRequest,
    ) -> super::transport::ApiFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((account.clone(), request));
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(GitHubError::Network)
        })
    }
}
fn response(body: Value, more: bool) -> ApiResponse {
    ApiResponse {
        status: 200,
        body,
        headers: if more {
            BTreeMap::from([(
                "link".into(),
                "<https://attacker.invalid/ignored>; rel=\"next\"".into(),
            )])
        } else {
            BTreeMap::new()
        },
    }
}
fn fixture(responses: Vec<ApiResponse>) -> (GitHubClient, Arc<Fixture>) {
    let fixture = Arc::new(Fixture {
        responses: Mutex::new(responses.into()),
        calls: Mutex::new(Vec::new()),
    });
    (
        GitHubClients::with_transport(fixture.clone())
            .for_account(AccountScope::new("Second", "team").unwrap()),
        fixture,
    )
}
fn repository() -> RepositoryRef {
    RepositoryRef::parse("team/private").unwrap()
}
fn issue(number: u64) -> Value {
    json!({"number":number,"title":"Fixture issue","state":"open","created_at":"2026-10-15T00:00:00Z","updated_at":"2026-10-15T00:00:00Z","user":{"login":"author"},"labels":[{"name":"bug"}],"assignees":[],"comments":0})
}

#[tokio::test]
async fn issue_pages_and_details_accept_mixed_string_and_object_labels() {
    let mut mixed = issue(13);
    mixed["labels"] = json!(["legacy-label", {"name":"object-label"}]);
    let mut malformed = mixed.clone();
    malformed["labels"] = json!([true]);
    let (client, _) = fixture(vec![
        response(json!([mixed.clone()]), false),
        response(mixed, false),
        response(malformed, false),
    ]);
    let page = client
        .repository_issues(&repository(), false, None, true)
        .await
        .unwrap();
    assert_eq!(page.items[0].labels, ["legacy-label", "object-label"]);
    let detail = client.issue(&repository(), 13, true).await.unwrap();
    assert_eq!(detail.issue.labels, ["legacy-label", "object-label"]);
    assert!(matches!(
        client.issue(&repository(), 13, true).await,
        Err(GitHubError::InvalidData)
    ));
}

#[tokio::test]
async fn profile_searches_keep_all_open_authored_prs_and_separate_recent_merges() {
    let empty = || {
        response(
            json!({"data":{"search":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}),
            false,
        )
    };
    let (client, fixture) = fixture(vec![
        response(json!({"type":"Organization"}), false),
        empty(),
        empty(),
        empty(),
    ]);
    client
        .user_pull_requests("author", false, false)
        .await
        .unwrap();
    client
        .user_pull_requests("author", true, false)
        .await
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let queries: Vec<_> = calls
        .iter()
        .filter_map(|(_, request)| {
            request
                .body
                .as_ref()
                .and_then(|body| body["variables"]["query"].as_str())
        })
        .collect();
    assert_eq!(
        queries.len(),
        3,
        "authored open, authored merged, and reviewed are separate searches"
    );
    assert!(queries[0].contains("author:author") && queries[0].contains("is:open"));
    assert!(
        !queries[0].contains("updated:") && !queries[0].contains("merged:"),
        "stale open authored PRs must remain visible"
    );
    assert!(
        queries[1].contains("author:author")
            && queries[1].contains("is:merged")
            && queries[1].contains("merged:>=")
    );
    assert!(queries[2].contains("reviewed-by:author") && queries[2].contains("-author:author"));
    assert!(
        !queries[2].contains("updated:>="),
        "reviewed profiles retain the incumbent unbounded date range"
    );
}
#[tokio::test]
async fn profile_search_failures_are_visible_without_losing_successful_results() {
    let empty = response(
        json!({"data":{"search":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}),
        false,
    );
    let (client, _) = fixture(vec![response(json!({"type":"Organization"}), false), empty]);
    let partial = client
        .user_pull_requests("author", false, true)
        .await
        .unwrap();
    assert_eq!(partial.warnings, [GitHubError::Network]);
    let (client, _) = fixture(vec![response(json!({"type":"Organization"}), false)]);
    assert!(matches!(
        client.user_pull_requests("author", false, true).await,
        Err(GitHubError::Network)
    ));
}

#[tokio::test]
async fn issue_pagination_survives_a_page_containing_only_pull_requests() {
    let (client, fixture) = fixture(vec![
        response(json!([{"pull_request":{}}]), true),
        response(json!([issue(13)]), false),
    ]);
    let first = client
        .repository_issues(&repository(), false, None, false)
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next, Some(PageCursor::Rest(2)));
    let second = client
        .repository_issues(&repository(), false, first.next.as_ref(), false)
        .await
        .unwrap();
    assert_eq!(second.items[0].number, 13);
    assert_eq!(second.items[0].author, "author");
    assert!(second.next.is_none());
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].1.path.starts_with("/repos/team/private/issues?"));
    assert!(calls[1].1.path.ends_with("page=2"));
    assert!(
        calls
            .iter()
            .all(|(account, _)| account.username() == "Second")
    );
}
#[tokio::test]
async fn rest_page_limit_is_explicit_and_untrusted_next_links_are_never_followed() {
    let (client, fixture) = fixture(vec![response(json!([]), true)]);
    let last = client
        .repository_issues(&repository(), true, Some(&PageCursor::Rest(100)), false)
        .await
        .unwrap();
    assert!(last.truncated);
    assert!(last.next.is_none());
    assert!(
        fixture.calls.lock().unwrap()[0]
            .1
            .path
            .contains("state=closed")
    );
    assert!(matches!(
        client
            .repository_issues(&repository(), false, Some(&PageCursor::Rest(101)), false)
            .await,
        Err(GitHubError::InvalidResource)
    ));
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn thread_comments_reject_a_global_node_that_belongs_to_another_pull() {
    let pull = PullRef::new(repository(), 7).unwrap();
    let thread = ThreadRef {
        pull,
        id: "PRRT_fixture".into(),
    };
    let (client, _) = fixture(vec![response(
        json!({"data":{"node":{"pullRequest":{"number":8,"repository":{"nameWithOwner":"team/private"}},"comments":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}),
        false,
    )]);
    assert!(matches!(
        client.thread_comments(&thread, None, false).await,
        Err(GitHubError::InvalidResource)
    ));
}
#[tokio::test]
async fn thread_comment_pagination_keeps_the_bound_resource_and_rejects_repeated_cursors() {
    let thread = ThreadRef {
        pull: PullRef::new(repository(), 7).unwrap(),
        id: "PRRT_fixture".into(),
    };
    let node = json!({"data":{"node":{"pullRequest":{"number":7,"repository":{"nameWithOwner":"team/private"}},"comments":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"cursor-one"}}}}});
    let (client, fixture) = fixture(vec![response(node.clone(), false), response(node, false)]);
    let first = client.thread_comments(&thread, None, false).await.unwrap();
    assert_eq!(first.next, Some(PageCursor::GraphQl("cursor-one".into())));
    assert!(matches!(
        client
            .thread_comments(&thread, first.next.as_ref(), false)
            .await,
        Err(GitHubError::InvalidData)
    ));
    assert_eq!(
        fixture.calls.lock().unwrap()[1].1.body.as_ref().unwrap()["variables"]["after"],
        "cursor-one"
    );
}
#[tokio::test]
async fn accurate_issue_and_pull_counts_do_not_use_the_combined_rest_open_items_count() {
    let (client, fixture) = fixture(vec![response(
        json!({"data":{"repository":{"issues":{"totalCount":3},"pullRequests":{"totalCount":9}}}}),
        false,
    )]);
    assert_eq!(
        client
            .repository_counts(&repository(), false)
            .await
            .unwrap(),
        (3, 9)
    );
    let calls = fixture.calls.lock().unwrap();
    let variables = &calls[0].1.body.as_ref().unwrap()["variables"];
    assert_eq!(variables["owner"], "team");
    assert_eq!(variables["repo"], "private");
}
#[tokio::test]
async fn invalid_contribution_dates_are_rejected_instead_of_rendered_as_calendar_data() {
    let (client, _) = fixture(vec![response(
        json!({"data":{"user":{"contributionsCollection":{"contributionCalendar":{"totalContributions":1,"weeks":[{"contributionDays":[{"date":"2026-02-30","contributionCount":1}]}]}}}}}),
        false,
    )]);
    assert!(matches!(
        client.user_contributions("author", false).await,
        Err(GitHubError::InvalidData)
    ));
}
#[tokio::test]
async fn personal_repository_loading_uses_the_authenticated_private_repository_endpoint() {
    let (client, fixture) = fixture(vec![
        response(json!({"type":"User"}), false),
        response(json!([]), true),
    ]);
    let page = client.repositories(None, false).await.unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.next, Some(PageCursor::Rest(2)));
    let calls = fixture.calls.lock().unwrap();
    assert!(
        calls[1]
            .1
            .path
            .starts_with("/user/repos?visibility=all&affiliation=owner,collaborator")
    );
    assert!(
        calls
            .iter()
            .all(|(account, _)| account.username() == "Second")
    );
}
