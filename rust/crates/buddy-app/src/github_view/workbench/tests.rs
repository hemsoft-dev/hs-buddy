use super::*;
use buddy_core::github::{ApiRequest, ApiResponse, Transport};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// A quota above the limit makes the fixture fail its `/rate_limit` read.
const FAIL: u64 = u64::MAX;

struct OverviewFixture {
    personal: bool,
    remaining: Mutex<u64>,
    /// The quota read is throttled with this `Retry-After`.
    throttled: Mutex<Option<u64>>,
    /// The quota read and searches never answer.
    stalled: bool,
    paths: Mutex<Vec<String>>,
}

impl OverviewFixture {
    fn new(personal: bool, remaining: u64) -> Arc<Self> {
        Arc::new(Self {
            personal,
            remaining: Mutex::new(remaining),
            throttled: Mutex::new(None),
            stalled: false,
            paths: Mutex::new(Vec::new()),
        })
    }

    fn stalled() -> Arc<Self> {
        Arc::new(Self {
            stalled: true,
            ..Arc::into_inner(Self::new(true, 4321)).unwrap()
        })
    }

    fn reads(&self, prefix: &str) -> usize {
        let paths = self.paths.lock().unwrap();
        paths.iter().filter(|path| path.starts_with(prefix)).count()
    }
}

fn search_item(number: u64) -> Value {
    json!({"repository_url":"https://api.github.com/repos/team/alpha","number":number,
        "title":"Fixture workbench issue","state":"open","updated_at":"2026-10-15T12:00:00Z"})
}

impl Transport for OverviewFixture {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        request: ApiRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApiResponse, GitHubError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let path = request.path.clone();
            self.paths.lock().unwrap().push(path.clone());
            if self.stalled && (path == "/rate_limit" || path.starts_with("/search/")) {
                std::future::pending::<()>().await;
            }
            let remaining = *self.remaining.lock().unwrap();
            let mut headers = BTreeMap::from([("x-ratelimit-remaining".into(), "0".into())]);
            let throttled = *self.throttled.lock().unwrap();
            let (status, body) = if path == "/users/team" {
                let kind = if self.personal {
                    "User"
                } else {
                    "Organization"
                };
                (200, json!({"login":"team","type":kind}))
            } else if path.starts_with("/user/repos") || path.starts_with("/orgs/team/repos") {
                (
                    200,
                    json!([{"full_name":"team/Alpha","private":true,"archived":false,"fork":false,
                        "stargazers_count":1,"forks_count":0,"open_issues_count":1,"size":1,
                        "updated_at":"2026-10-15T12:00:00Z"}]),
                )
            } else if let (true, Some(retry)) = (path == "/rate_limit", throttled) {
                headers.insert("retry-after".into(), retry.to_string());
                (429, Value::Null)
            } else if path == "/rate_limit" && remaining != FAIL {
                let reset = Timestamp::now().as_second() + 600;
                (
                    200,
                    json!({"resources":{"core":{"limit":5000,"remaining":remaining,"used":5000 - remaining,"reset":reset}}}),
                )
            } else if path.contains("is%3Aissue") {
                (200, json!({"total_count":1,"items":[search_item(11)]}))
            } else if path.contains("is%3Apr") {
                (403, Value::Null)
            } else {
                (500, Value::Null)
            };
            Ok(ApiResponse {
                status,
                headers,
                body,
            })
        })
    }
}

fn overview(
    cx: &mut gpui_kit::TestAppContext,
    fixture: Arc<OverviewFixture>,
) -> Entity<GitHubView> {
    let mut entity = None;
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.github.accounts = vec![buddy_core::config::GitHubAccount {
            username: "First".into(),
            org: "team".into(),
            repo_root: None,
            usage_provider: None,
            extra: BTreeMap::new(),
        }];
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.clients = GitHubClients::with_transport(fixture);
                view.active = true;
                view.change_account(Some(AccountScope::new("First", "team").unwrap()), cx);
                view.navigate(Route::Organization, cx);
                view
            });
            entity = Some(view.clone());
            view
        })
        .unwrap();
    });
    cx.run_until_parked();
    entity.unwrap()
}

fn remaining(cx: &mut gpui_kit::TestAppContext, view: &Entity<GitHubView>) -> Option<u64> {
    cx.update(|cx| {
        view.read(cx)
            .overview
            .rate_limit
            .ready()
            .map(|limit| limit.remaining)
    })
}

/// Make the next 5-second tick due for a quota read, then run that tick.
fn tick(
    cx: &mut gpui_kit::TestAppContext,
    view: &Entity<GitHubView>,
    before_reply: impl FnOnce(&mut GitHubView, &mut Context<GitHubView>),
) {
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.overview.rate_limit_read_at = Instant::now().checked_sub(RATE_LIMIT_REFRESH);
            view.poll_rate_limit(cx);
            before_reply(view, cx);
        })
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_personal_overview_shows_the_quota_gauge_and_ranks_its_workbench(
    cx: &mut gpui_kit::TestAppContext,
) {
    let fixture = OverviewFixture::new(true, 4321);
    let view = overview(cx, fixture.clone());
    assert_eq!(remaining(cx, &view), Some(4321));
    cx.update(|cx| {
        let view = view.read(cx);
        let data = view.data.as_ref().unwrap();
        let activity = view
            .overview
            .workbench
            .ready()
            .expect("a personal namespace has a workbench");
        let alpha = &activity.repositories[0];
        assert_eq!(alpha.name(), "Alpha");
        assert_eq!(alpha.issues[0].number, 11);
        assert_eq!(alpha.issues[0].url, "https://github.com/team/alpha/issues/11");
        assert_eq!(
            lane_summary(&alpha.issues, &activity.issues_error, false),
            ("1".to_string(), None)
        );
        // The limited pull-request search is unknown, not an empty lane.
        assert_eq!(
            lane_summary(&alpha.pull_requests, &activity.pull_requests_error, false),
            ("!".to_string(), Some("Activity unavailable"))
        );
        assert!(
            status(activity)
                .unwrap()
                .starts_with("Showing the activity GitHub returned. One side could not be refreshed. Pull requests: GitHub is limiting requests")
        );
        assert_eq!(
            header_meta(activity, activity.fetched_at),
            ["1 repository", "Updated just now"]
        );
        // The repository list below the workbench is unchanged.
        assert_eq!(data.rows[0].title, "team/Alpha");
    });
    assert_eq!(fixture.reads("/search/issues"), 2);
}

#[gpui_kit::test]
fn an_organization_overview_has_no_workbench_and_an_unread_quota_has_no_gauge(
    cx: &mut gpui_kit::TestAppContext,
) {
    let fixture = OverviewFixture::new(false, FAIL);
    let view = overview(cx, fixture.clone());
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(
            view.error.is_none(),
            "a quota failure does not fail the overview"
        );
        let data = view.data.as_ref().unwrap();
        assert!(matches!(view.overview.rate_limit, Section::Failed));
        assert!(matches!(view.overview.workbench, Section::Hidden));
        assert_eq!(data.rows.len(), 1);
    });
    // Electron searches activity for user namespaces only.
    assert_eq!(fixture.reads("/search/issues"), 0);
    assert_eq!(fixture.reads("/rate_limit"), 1);
}

#[gpui_kit::test]
fn the_quota_is_re_read_each_minute_for_the_shown_overview_only(cx: &mut gpui_kit::TestAppContext) {
    let fixture = OverviewFixture::new(true, 4321);
    let view = overview(cx, fixture.clone());
    assert_eq!(fixture.reads("/rate_limit"), 1);
    // The overview's own read counts: no second read inside the minute.
    cx.update(|cx| view.update(cx, |view, cx| view.poll_rate_limit(cx)));
    cx.run_until_parked();
    assert_eq!(fixture.reads("/rate_limit"), 1);

    *fixture.remaining.lock().unwrap() = 1200;
    tick(cx, &view, |_, _| {});
    assert_eq!(fixture.reads("/rate_limit"), 2);
    assert_eq!(remaining(cx, &view), Some(1200));

    // A reading requested before a reload is dropped; the reload's own
    // read reuses the cached 1,200.
    *fixture.remaining.lock().unwrap() = 900;
    tick(cx, &view, |view, cx| view.load(false, false, cx));
    assert_eq!(fixture.reads("/rate_limit"), 3);
    assert_eq!(remaining(cx, &view), Some(1200));
    assert!(cx.update(|cx| !view.read(cx).overview.rate_limit_loading));

    // A failed re-read hides the gauge instead of keeping an old quota.
    *fixture.remaining.lock().unwrap() = FAIL;
    tick(cx, &view, |_, _| {});
    assert_eq!(remaining(cx, &view), None);

    // A reading for a page left before it arrived is dropped.
    *fixture.remaining.lock().unwrap() = 800;
    tick(cx, &view, |view, cx| view.navigate(Route::Members, cx));
    assert_eq!(remaining(cx, &view), None);

    // Another page never polls the quota.
    let before = fixture.reads("/rate_limit");
    tick(cx, &view, |_, _| {});
    assert_eq!(fixture.reads("/rate_limit"), before);
}

#[gpui_kit::test]
fn the_overview_shows_its_repositories_while_the_quota_and_workbench_are_still_reading(
    cx: &mut gpui_kit::TestAppContext,
) {
    let fixture = OverviewFixture::stalled();
    let view = overview(cx, fixture.clone());
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.error.is_none() && !view.loading);
        assert_eq!(view.data.as_ref().unwrap().rows[0].title, "team/Alpha");
        assert!(matches!(view.overview.rate_limit, Section::Loading));
        assert!(matches!(view.overview.workbench, Section::Loading));
    });
    assert_eq!(fixture.reads("/search/issues"), 2);
    // A quota read still in progress is not repeated by the poll.
    tick(cx, &view, |_, _| {});
    assert_eq!(fixture.reads("/rate_limit"), 1);
}

#[gpui_kit::test]
fn a_throttled_quota_read_waits_for_githubs_retry_window(cx: &mut gpui_kit::TestAppContext) {
    let fixture = OverviewFixture::new(false, 4321);
    let view = overview(cx, fixture.clone());
    *fixture.throttled.lock().unwrap() = Some(300);
    tick(cx, &view, |_, _| {});
    assert_eq!(fixture.reads("/rate_limit"), 2);
    assert_eq!(remaining(cx, &view), None);
    let (account, retry_at) =
        cx.update(|cx| view.read(cx).overview.rate_limit_retry.clone().unwrap());
    assert_eq!(account.organization(), "team");
    assert!(retry_at >= Instant::now() + Duration::from_secs(290));

    // The minute has passed, but GitHub's window has not.
    tick(cx, &view, |_, _| {});
    assert_eq!(fixture.reads("/rate_limit"), 2);
    // Reopening the overview inside the window does not read it either.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.navigate(Route::Members, cx);
            view.navigate(Route::Organization, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(fixture.reads("/rate_limit"), 2);

    // Once the window has passed, the next poll reads the quota again.
    *fixture.throttled.lock().unwrap() = None;
    tick(cx, &view, |view, cx| {
        if let Some((_, at)) = &mut view.overview.rate_limit_retry {
            *at = Instant::now();
        }
        view.poll_rate_limit(cx);
    });
    assert_eq!(fixture.reads("/rate_limit"), 3);
    assert_eq!(remaining(cx, &view), Some(4321));
    assert!(cx.update(|cx| view.read(cx).overview.rate_limit_retry.is_none()));
}

#[test]
fn a_throttled_quota_read_waits_at_least_a_minute_and_until_an_exhausted_reset() {
    let limited = |retry_after_seconds| GitHubError::RateLimited {
        retry_after_seconds,
    };
    let reading = |remaining| RateLimit {
        limit: 5000,
        remaining,
        used: 5000 - remaining,
        reset: 1_900,
    };
    let secs = Duration::from_secs;
    assert_eq!(quota_retry_delay(&limited(5), None, 1_000), Some(secs(60)));
    assert_eq!(
        quota_retry_delay(&limited(300), None, 1_000),
        Some(secs(300))
    );
    // A quota last seen exhausted is not read again before it resets.
    assert_eq!(
        quota_retry_delay(&limited(30), Some(&reading(0)), 1_000),
        Some(secs(900))
    );
    assert_eq!(
        quota_retry_delay(&limited(30), Some(&reading(10)), 1_000),
        Some(secs(60))
    );
    assert_eq!(
        quota_retry_delay(&limited(30), Some(&reading(0)), 5_000),
        Some(secs(60))
    );
    // Other failures retry on the ordinary minute.
    assert_eq!(quota_retry_delay(&GitHubError::Network, None, 1_000), None);
}

#[gpui_kit::test]
fn reopening_the_overview_keeps_a_cached_quotas_read_time(cx: &mut gpui_kit::TestAppContext) {
    let fixture = OverviewFixture::new(false, 4321);
    let view = overview(cx, fixture.clone());
    let first = cx.update(|cx| view.read(cx).overview.rate_limit_read_at.unwrap());
    std::thread::sleep(Duration::from_millis(300));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.navigate(Route::Members, cx);
            view.navigate(Route::Organization, cx);
        })
    });
    cx.run_until_parked();
    // The cached reading is reused and keeps its age.
    assert_eq!(fixture.reads("/rate_limit"), 1);
    assert_eq!(remaining(cx, &view), Some(4321));
    let reopened = cx.update(|cx| view.read(cx).overview.rate_limit_read_at.unwrap());
    assert!(reopened < first + Duration::from_millis(150));
}

#[gpui_kit::test]
fn a_workbench_reply_is_applied_only_to_the_newest_read_of_the_shown_overview(
    cx: &mut gpui_kit::TestAppContext,
) {
    let fixture = OverviewFixture::new(true, 4321);
    let view = overview(cx, fixture.clone());
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let account = view.account.clone().unwrap();
            let request = view.overview.workbench_request;
            assert!(view.accepts_workbench(&account, request));
            assert!(
                !view.accepts_workbench(&AccountScope::new("Second", "team").unwrap(), request)
            );
            view.load_workbench(Vec::new(), true, cx);
            assert!(!view.accepts_workbench(&account, request));
            // A read still in progress when the overview is left is dropped.
            view.navigate(Route::Members, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(matches!(view.read(cx).overview.workbench, Section::Hidden)));
}

#[test]
fn the_gauge_colour_and_text_follow_electrons_rate_limit_gauge() {
    assert_eq!(gauge_color(1.0), 0x50c850);
    assert_eq!(gauge_color(0.6), 0xdcaa32);
    assert_eq!(gauge_color(0.45), 0xe69132);
    assert_eq!(gauge_color(0.0), 0xf00028);
    assert_eq!(gauge_color(-1.0), gauge_color(0.0));
    let limit = RateLimit {
        limit: 5000,
        remaining: 4321,
        used: 679,
        reset: 1_600,
    };
    assert_eq!(
        gauge_text(&limit, 1_000),
        (
            buddy_core::format::thousands(4321),
            format!("/ {} · resets 10m", buddy_core::format::thousands(5000))
        )
    );
}

#[test]
fn lanes_and_items_read_like_electrons_workbench() {
    let item = ActivityItem {
        number: 4,
        title: "Fixture".into(),
        url: "https://github.com/team/alpha/pull/4".into(),
        state: github::ActivityState::Draft,
        updated_at: "2026-10-13T12:00:00Z".parse().unwrap(),
    };
    let now: Timestamp = "2026-10-15T12:00:00Z".parse().unwrap();
    assert_eq!(item_meta(&item, now), "draft · 2 days ago");
    assert_eq!(
        lane_summary(&[], &None, false),
        ("0".to_string(), Some("Nothing recent"))
    );
    assert_eq!(
        lane_summary(
            std::slice::from_ref(&item),
            &Some(GitHubError::Network),
            false
        ),
        ("!".to_string(), Some("Activity unavailable"))
    );
    // A timed-out search is never "Nothing recent" or an exact count.
    assert_eq!(
        lane_summary(&[], &None, true),
        ("!".to_string(), Some("Search incomplete"))
    );
    assert_eq!(
        lane_summary(&[item], &None, true),
        ("1+".to_string(), Some("Search incomplete; more may exist"))
    );
}

#[test]
fn an_unfinished_search_never_claims_there_was_no_recent_activity() {
    let now: Timestamp = "2026-10-15T12:00:00Z".parse().unwrap();
    let activity = |issues_error, issues_incomplete, repositories| RepositoryActivity {
        repositories,
        issues_error,
        pull_requests_error: None,
        issues_incomplete,
        pull_requests_incomplete: false,
        has_more: false,
        fetched_at: now,
    };
    let complete = activity(None, false, Vec::new());
    assert_eq!(
        status(&complete).as_deref(),
        Some("No recent issue or pull-request activity found.")
    );
    assert_eq!(
        header_meta(&complete, now),
        ["0 repositories", "Updated just now"]
    );

    // A timed-out search with nothing to show is reported like a failed lane.
    let unread = activity(Some(GitHubError::IncompleteSearch), false, Vec::new());
    assert_eq!(
        status(&unread).as_deref(),
        Some(
            "Showing the activity GitHub returned. One side could not be refreshed. Issues: GitHub's search timed out before it finished. Refresh to search again."
        )
    );
    assert_eq!(header_meta(&unread, now), ["Updated just now"]);

    let alpha = github::ActiveRepository {
        repository: github::RepositoryRef::parse("team/alpha").unwrap(),
        full_name: "team/alpha".into(),
        updated_at: now,
        issues: Vec::new(),
        pull_requests: Vec::new(),
    };
    let partial = activity(None, true, vec![alpha]);
    assert_eq!(
        status(&partial).as_deref(),
        Some("GitHub's search timed out before it finished, so this activity may be incomplete.")
    );
    assert_eq!(
        header_meta(&partial, now),
        ["1 repository", "Updated just now"]
    );
}
