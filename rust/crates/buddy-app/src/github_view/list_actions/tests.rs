use super::super::tests::{account, bookmark, setup, wait_for_write};
use super::*;
use buddy_core::github::{PullSummary, RepositoryRef};
use gpui_kit::{MouseButton, VisualTestContext, px};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

const HEAD: &str = "abcdef0123456789abcdef0123456789abcdef01";

fn target() -> PullRef {
    PullRef::new(RepositoryRef::parse("team/private").unwrap(), 7).unwrap()
}

fn summary(state: &str, i_approved: bool) -> PullSummary {
    PullSummary {
        target: target(),
        title: "Listed fixture review".into(),
        author: "fixture-author".into(),
        body: None,
        state: state.into(),
        created_at: "2026-10-15T12:00:00Z".into(),
        updated_at: "2026-10-15T12:00:00Z".into(),
        merged_at: None,
        head_branch: "feature".into(),
        base_branch: "main".into(),
        head_sha: HEAD.into(),
        draft: false,
        i_approved,
        approval_count: Some(0),
        review_decision: None,
        thread_count: 0,
        changed_files: 1,
    }
}

/// GitHub for one PR: its head read, then the write, which is accepted or
/// rejected as GitHub rejects approving your own PR.
#[derive(Default)]
struct Fixture {
    calls: Mutex<Vec<github::ApiRequest>>,
    reject: bool,
    /// The write's outcome is lost, as when the connection drops after it.
    unknown: bool,
}

impl github::Transport for Fixture {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        request: github::ApiRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<github::ApiResponse, GitHubError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request.clone());
            if request.is_write && self.unknown {
                return Err(GitHubError::UnknownWriteResult);
            }
            let (status, body) = if request.is_write && self.reject {
                (
                    422,
                    serde_json::json!({"message":"Can not approve your own pull request"}),
                )
            } else if request.is_write && request.path.ends_with("/reviews") {
                let commit = request.body.as_ref().unwrap()["commit_id"].clone();
                (
                    200,
                    serde_json::json!({"id":302,"state":"APPROVED","commit_id":commit}),
                )
            } else if request.is_write {
                (201, serde_json::json!({"id":7}))
            } else {
                (
                    200,
                    serde_json::json!({"data":{"repository":{"pullRequest":{
                        "number":7,"title":"Listed fixture review","state":"OPEN","body":"",
                        "createdAt":"2026-10-15T12:00:00Z","updatedAt":"2026-10-15T12:00:00Z",
                        "mergedAt":null,"headRefName":"feature","baseRefName":"main",
                        "headRefOid":HEAD,"changedFiles":1,"isDraft":false,"reviewDecision":null,
                        "author":{"login":"fixture-author"},
                        "repository":{"nameWithOwner":"team/private"},
                        "latestOpinion":{"nodes":[]},"reviewThreads":{"totalCount":0},
                        "recentReviews":{"nodes":[],"pageInfo":{"hasPreviousPage":false}}
                    }}}}),
                )
            };
            Ok(github::ApiResponse {
                status,
                headers: BTreeMap::new(),
                body,
            })
        })
    }
}

fn shown(view: &mut GitHubView, rows: Vec<data::Row>) {
    view.data = Some(PageData {
        title: "Needs Review".into(),
        rows,
        ..Default::default()
    });
}

fn list_view(
    cx: &mut gpui_kit::TestAppContext,
    transport: Arc<Fixture>,
    pull: PullSummary,
) -> Entity<GitHubView> {
    let mut entity = None;
    cx.update(|cx| {
        setup(cx);
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.clients = GitHubClients::with_transport(transport);
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
                shown(&mut view, vec![data::pull_row(pull)]);
                view
            });
            entity = Some(view.clone());
            view
        })
        .unwrap();
    });
    entity.unwrap()
}

fn listed(view: &GitHubView) -> ListedPull {
    let row = &view.data.as_ref().unwrap().rows[0];
    view.listed(row).unwrap().clone()
}

/// The row's identity, as its open menu holds it.
fn row(view: &GitHubView) -> RowTarget {
    view.row_target(&listed(view)).unwrap()
}

fn writes(transport: &Fixture) -> Vec<github::ApiRequest> {
    transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.is_write)
        .cloned()
        .collect()
}

fn labels(entries: &[MenuEntry]) -> Vec<(&str, bool)> {
    entries
        .iter()
        .map(|entry| (entry.label.as_str(), entry.enabled))
        .collect()
}

#[gpui_kit::test]
fn a_row_approves_its_listed_head_only_after_the_confirmation(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let pull = listed(view);
            assert_eq!(view.approve_control(&pull), ("Approve…", true));
            view.run_row_action(&row(view), RowAction::Approve, cx);
            // The PR page's confirmation, pinned to the head the list showed.
            match view.pending.as_ref().unwrap().action() {
                WriteAction::Approve { pull, head_sha } => {
                    assert_eq!(pull, &target());
                    assert_eq!(head_sha, HEAD);
                }
                other => panic!("unexpected {other:?}"),
            }
            assert!(view.pending_list.is_some());
            assert_eq!(
                view.approve_control(&pull),
                ("Approve…", false),
                "an open confirmation holds the row's other writes back"
            );
            assert!(
                transport.calls.lock().unwrap().is_empty(),
                "nothing is sent before confirming"
            );
            view.confirm(cx);
            assert_eq!(
                view.row_write(&pull),
                Some(&RowWrite::Running("Approving…"))
            );
        })
    });
    wait_for_write(cx, &view);
    let sent = writes(&transport);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].path, "/repos/team/private/pulls/7/reviews");
    let body = sent[0].body.as_ref().unwrap();
    assert_eq!(body["event"], "APPROVE");
    assert_eq!(body["commit_id"], HEAD);
    // The head is read again before approving, as from the PR page.
    assert!(
        transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| !call.is_write
                && call.body.as_ref().is_some_and(|body| body["query"]
                    .as_str()
                    .is_some_and(|query| query.contains("NativePull("))))
    );
    cx.update(|cx| {
        let view = view.read(cx);
        let pull = listed(view);
        assert!(pull.i_approved, "the row is marked approved, as Electron's");
        assert_eq!(view.approve_control(&pull), ("Approved", false));
        assert_eq!(
            view.row_write(&pull),
            Some(&RowWrite::Done("Commit approved"))
        );
        assert_eq!(
            labels(&view.menu_entries(&pull))[2],
            ("Already Approved", false)
        );
        assert!(view.pending.is_none() && view.pending_list.is_none());
    });
}

#[gpui_kit::test]
fn a_rejected_row_approval_is_shown_on_its_row_and_leaves_it_unapproved(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Arc::new(Fixture {
        reject: true,
        ..Default::default()
    });
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), RowAction::Approve, cx);
            view.confirm(cx);
        })
    });
    wait_for_write(cx, &view);
    assert_eq!(writes(&transport).len(), 1);
    cx.update(|cx| {
        let view = view.read(cx);
        let pull = listed(view);
        assert!(!pull.i_approved);
        assert_eq!(
            view.row_write(&pull),
            Some(&RowWrite::Failed(GitHubError::Rejected.to_string()))
        );
        assert_eq!(view.approve_control(&pull), ("Approve…", true));
        // The outcome is the row's; the page status is not reused for it.
        assert!(view.status.is_none());
    });
}

#[gpui_kit::test]
fn a_row_confirmation_is_refused_once_its_list_is_read_again(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let pull = listed(view);
            view.run_row_action(&row(view), RowAction::Approve, cx);
            // A newer read of the list: the reviewed row may no longer be shown.
            view.selection.begin();
            view.confirm(cx);
            assert_eq!(
                view.status.as_deref(),
                Some(GitHubError::StaleConfirmation.to_string().as_str())
            );
            assert!(!view.write_busy);
            assert!(view.row_write(&pull).is_none());
        })
    });
    cx.run_until_parked();
    assert!(transport.calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn a_page_confirmation_after_a_row_one_is_confirmed_as_a_page_write(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport, summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), RowAction::RequestCopilot, cx);
            assert!(view.pending_list.is_some());
            view.navigate(Route::Pull(target(), PullTab::Conversation), cx);
            assert!(view.pending.is_none());
            view.prepare(
                WriteAction::Approve {
                    pull: target(),
                    head_sha: HEAD.into(),
                },
                cx,
            );
            assert!(view.pending.is_some());
            assert!(
                view.pending_list.is_none(),
                "a page write is never checked against an earlier list"
            );
            // Row actions belong to the PR lists only.
            assert!(
                view.listed(&data::pull_row(summary("OPEN", false)))
                    .is_none()
            );
            view.prepare_listed(WriteAction::RequestCopilot { pull: target() }, cx);
            assert!(view.pending_list.is_none());
        })
    });
}

#[gpui_kit::test]
fn the_row_menu_has_electrons_entries_in_order_with_their_conditions(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = list_view(cx, Arc::new(Fixture::default()), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let pull = listed(view);
            // Bookmarks are not loaded yet: the repository page disables its control too.
            assert_eq!(
                labels(&view.menu_entries(&pull)),
                vec![
                    ("Request AI Review…", true),
                    ("Request Copilot Review…", true),
                    ("Approve…", true),
                    ("Copy Link", true),
                    ("Bookmark private", false),
                ]
            );
            view.bookmarks = Some(vec![bookmark("team", "other")]);
            assert_eq!(
                labels(&view.menu_entries(&pull))[4],
                ("Bookmark private", true)
            );
            view.bookmarks = Some(vec![bookmark("team", "private")]);
            assert_eq!(
                labels(&view.menu_entries(&pull))[4],
                ("Unbookmark private", true)
            );

            let approved = ListedPull {
                i_approved: true,
                ..pull.clone()
            };
            assert_eq!(
                labels(&view.menu_entries(&approved))[2],
                ("Already Approved", false)
            );
            // As on the PR page, a merged or closed PR offers no review writes.
            let merged = ListedPull {
                open: false,
                ..pull.clone()
            };
            assert_eq!(
                labels(&view.menu_entries(&merged)),
                vec![
                    ("Request AI Review…", true),
                    ("Request Copilot Review…", false),
                    ("Approve…", false),
                    ("Copy Link", true),
                    ("Unbookmark private", true),
                ]
            );
            assert_eq!(view.approve_control(&merged), ("Approve…", false));
            // The card has the button; Electron's list view has only the menu.
            assert_eq!(view.row_approve(&pull), Some(("Approve…", true)));
            view.view_mode = ViewMode::List;
            assert_eq!(view.row_approve(&pull), None);
            assert_eq!(labels(&view.menu_entries(&pull))[2], ("Approve…", true));
            view.view_mode = ViewMode::Card;
            // A disabled entry does nothing even when reached directly.
            shown(view, vec![data::pull_row(summary("MERGED", false))]);
            view.run_row_action(&row(view), RowAction::Approve, cx);
            assert!(view.pending.is_none());
            shown(view, vec![data::pull_row(summary("OPEN", false))]);

            view.write_busy = true;
            let entries = view.menu_entries(&pull);
            let busy = labels(&entries);
            assert_eq!(busy[1], ("Request Copilot Review…", false));
            assert_eq!(busy[2], ("Approve…", false));
            assert_eq!(busy[4], ("Unbookmark private", false));
            view.write_busy = false;
        })
    });
}

#[gpui_kit::test]
fn copy_link_puts_the_pull_requests_address_on_the_clipboard(cx: &mut gpui_kit::TestAppContext) {
    let view = list_view(cx, Arc::new(Fixture::default()), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), RowAction::CopyLink, cx);
            assert_eq!(
                view.status.as_deref(),
                Some("Link copied · team/private #7")
            );
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("https://github.com/team/private/pull/7".into())
        );
    });
}

#[gpui_kit::test]
fn request_copilot_review_from_the_menu_confirms_then_requests_the_bot(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), RowAction::RequestCopilot, cx);
            assert!(matches!(
                view.pending.as_ref().unwrap().action(),
                WriteAction::RequestCopilot { .. }
            ));
            assert!(writes(&transport).is_empty());
            view.confirm(cx);
        })
    });
    wait_for_write(cx, &view);
    let sent = writes(&transport);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].path,
        "/repos/team/private/pulls/7/requested_reviewers"
    );
    assert_eq!(
        sent[0].body.as_ref().unwrap()["reviewers"][0],
        "copilot-pull-request-reviewer[bot]"
    );
    cx.update(|cx| {
        let view = view.read(cx);
        let pull = listed(view);
        assert_eq!(
            view.row_write(&pull),
            Some(&RowWrite::Done("Copilot review requested"))
        );
        assert!(!pull.i_approved);
    });
}

#[gpui_kit::test]
fn another_accounts_row_write_never_marks_the_shown_list(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport, summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), RowAction::Approve, cx);
            view.confirm(cx);
            // Another account's list shows the same PR before the write ends.
            view.change_account(Some(account("Second")), cx);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
        })
    });
    wait_for_write(cx, &view);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let pull = listed(view);
            assert!(!pull.i_approved);
            assert!(view.row_write(&pull).is_none());
            // The first account's row keeps its outcome.
            view.change_account(Some(account("First")), cx);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert_eq!(
                view.row_write(&listed(view)),
                Some(&RowWrite::Done("Commit approved"))
            );
        })
    });
}

#[gpui_kit::test]
fn right_clicking_a_row_opens_its_menu_and_request_ai_review_hands_it_over(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(setup);
    let (window, view) = cx.update(|cx| {
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.clients = GitHubClients::with_transport(Arc::new(Fixture::default()));
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
                shown(&mut view, vec![data::pull_row(summary("OPEN", false))]);
                view
            })
        })
        .unwrap()
    });
    let handed = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let handed = handed.clone();
        cx.subscribe(&view, move |_, event: &GitHubEvent, _| {
            if let GitHubEvent::PrepareAssistantReview(_, pull) = event {
                handed.borrow_mut().push(pull.clone());
            }
        })
        .detach();
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let row = visual.debug_bounds("github-item-row-0").unwrap();
    let press = row.origin + gpui_kit::point(px(40.), px(10.));
    visual.simulate_mouse_down(press, MouseButton::Right, Default::default());
    visual.simulate_mouse_up(press, MouseButton::Right, Default::default());
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    // The first entry, inside the menu's padding at the press.
    let item = press + gpui_kit::point(px(30.), px(17.));
    visual.simulate_mouse_move(item, None, Default::default());
    visual.simulate_click(item, Default::default());
    visual.run_until_parked();
    assert_eq!(*handed.borrow(), vec![target()]);
}

/// A successful forced read of the shown route, as its Refresh button gives.
fn forced_reload(view: &mut GitHubView) {
    let request = LoadContext {
        ticket: view.selection.begin().unwrap(),
        force: true,
        more: false,
        write_key: view.current_write_key(),
        can_unlock: true,
    };
    view.receive_load(request, Ok(PageData::default()));
}

/// Whether the row offers its GitHub writes, through every way to start one.
fn row_writes_enabled(view: &mut GitHubView, cx: &mut Context<GitHubView>) -> bool {
    let pull = listed(view);
    let entries = view.menu_entries(&pull);
    let approve = view.approve_control(&pull).1;
    assert_eq!(entries[1].enabled, approve, "Copilot follows Approve");
    assert_eq!(entries[2].enabled, approve, "the menu follows the button");
    assert_eq!(view.row_locked(&pull).is_none(), approve);
    view.prepare_listed(WriteAction::RequestCopilot { pull: target() }, cx);
    let prepared = view.pending.take().is_some();
    view.pending_list = None;
    assert_eq!(prepared, approve, "the last guard agrees with the controls");
    approve
}

/// Run a row write to its end, then check that only the PR page's recovery,
/// a forced read of the PR's Reviews tab, lets the row write again.
fn assert_locked_until_the_reviews_tab_is_reloaded(
    cx: &mut gpui_kit::TestAppContext,
    transport: Fixture,
    action: RowAction,
    outcome: RowWrite,
) {
    let transport = Arc::new(transport);
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.run_row_action(&row(view), action, cx);
            view.confirm(cx);
        })
    });
    wait_for_write(cx, &view);
    assert_eq!(writes(&transport).len(), 1);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let pull = listed(view);
            assert!(!pull.i_approved);
            assert_eq!(view.row_write(&pull), Some(&outcome));
            assert_eq!(view.row_locked(&pull), Some(ROW_LOCKED));
            assert_eq!(view.approve_control(&pull), ("Approve…", false));
            assert!(!row_writes_enabled(view, cx));
            // Reached directly, the entries still send nothing.
            view.run_row_action(&row(view), RowAction::Approve, cx);
            view.run_row_action(&row(view), RowAction::RequestCopilot, cx);
            assert!(view.pending.is_none());

            // Reloading the list cannot tell whether the write landed.
            forced_reload(view);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert!(!row_writes_enabled(view, cx));
            // Nor can the PR's conversation: the page keeps its lock too.
            view.navigate(Route::Pull(target(), PullTab::Conversation), cx);
            assert!(view.must_reload);
            forced_reload(view);
            assert!(view.must_reload);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert!(!row_writes_enabled(view, cx));

            // The Reviews tab shows the approval or request: it unlocks both.
            view.navigate(Route::Pull(target(), PullTab::Reviews), cx);
            forced_reload(view);
            assert!(!view.must_reload);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert!(row_writes_enabled(view, cx));
        })
    });
    cx.run_until_parked();
    assert_eq!(writes(&transport).len(), 1, "nothing was sent again");
}

#[gpui_kit::test]
fn an_uncertain_row_approval_locks_the_rows_writes_until_the_reviews_tab_is_reloaded(
    cx: &mut gpui_kit::TestAppContext,
) {
    assert_locked_until_the_reviews_tab_is_reloaded(
        cx,
        Fixture {
            unknown: true,
            ..Default::default()
        },
        RowAction::Approve,
        RowWrite::Failed(GitHubError::UnknownWriteResult.to_string()),
    );
}

#[gpui_kit::test]
fn an_uncertain_row_copilot_request_locks_the_rows_writes_until_the_reviews_tab_is_reloaded(
    cx: &mut gpui_kit::TestAppContext,
) {
    assert_locked_until_the_reviews_tab_is_reloaded(
        cx,
        Fixture {
            unknown: true,
            ..Default::default()
        },
        RowAction::RequestCopilot,
        RowWrite::Failed(GitHubError::UnknownWriteResult.to_string()),
    );
}

#[gpui_kit::test]
fn a_requested_copilot_review_locks_the_rows_writes_until_the_reviews_tab_is_reloaded(
    cx: &mut gpui_kit::TestAppContext,
) {
    // The request leaves the PR unapproved, so only the lock holds Copilot back.
    assert_locked_until_the_reviews_tab_is_reloaded(
        cx,
        Fixture::default(),
        RowAction::RequestCopilot,
        RowWrite::Done("Copilot review requested"),
    );
}

#[gpui_kit::test]
fn a_write_locked_on_the_pr_page_is_honoured_by_its_list_row(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.navigate(Route::Pull(target(), PullTab::Reviews), cx);
            view.prepare(
                WriteAction::Approve {
                    pull: target(),
                    head_sha: HEAD.into(),
                },
                cx,
            );
            view.confirm(cx);
        })
    });
    wait_for_write(cx, &view);
    assert_eq!(writes(&transport).len(), 1);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            assert!(view.must_reload);
            // Back on the list, read before the approval: the row still shows
            // the PR unapproved, and only the lock holds its writes back.
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            let pull = listed(view);
            assert!(!pull.i_approved);
            assert!(view.row_write(&pull).is_none(), "the write was the page's");
            assert!(!row_writes_enabled(view, cx));
            view.run_row_action(&row(view), RowAction::Approve, cx);
            assert!(view.pending.is_none());
            // Another account's list is not held back by this account's lock.
            view.change_account(Some(account("Second")), cx);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert!(row_writes_enabled(view, cx));
            view.change_account(Some(account("First")), cx);
            view.navigate(Route::Pull(target(), PullTab::Reviews), cx);
            forced_reload(view);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            assert!(row_writes_enabled(view, cx));
        })
    });
}

#[gpui_kit::test]
fn an_open_menu_acts_on_the_row_as_the_list_shows_it_now(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Fixture::default());
    let view = list_view(cx, transport.clone(), summary("OPEN", false));
    let handed = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let handed = handed.clone();
        cx.subscribe(&view, move |_, event: &GitHubEvent, _| {
            if let GitHubEvent::PrepareAssistantReview(client, pull) = event {
                handed
                    .borrow_mut()
                    .push((client.account.clone(), pull.clone()));
            }
        })
        .detach();
    });
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            // The menu opened on this row; the list is read again before a click.
            let open = row(view);
            shown(view, vec![data::pull_row(summary("OPEN", true))]);
            view.run_row_action(&open, RowAction::Approve, cx);
            assert!(view.pending.is_none(), "approved since the menu opened");
            shown(view, vec![data::pull_row(summary("MERGED", false))]);
            view.run_row_action(&open, RowAction::RequestCopilot, cx);
            assert!(view.pending.is_none(), "merged since the menu opened");
            // A newer head is the one approved.
            let newer = "1111111111111111111111111111111111111111";
            shown(
                view,
                vec![data::pull_row(PullSummary {
                    head_sha: newer.into(),
                    ..summary("OPEN", false)
                })],
            );
            view.run_row_action(&open, RowAction::Approve, cx);
            match view.pending.take().unwrap().action() {
                WriteAction::Approve { head_sha, .. } => assert_eq!(head_sha, newer),
                other => panic!("unexpected {other:?}"),
            }
            view.pending_list = None;
            // Gone from the list: nothing is done, and the row says why.
            shown(view, Vec::new());
            view.run_row_action(&open, RowAction::CopyLink, cx);
            assert_eq!(
                view.status.as_deref(),
                Some("This pull request is no longer in the shown list.")
            );
            // Another account's list holds the same PR: the menu is refused,
            // so the AI review never pairs this PR with that account.
            view.change_account(Some(account("Second")), cx);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            view.status = None;
            for action in [RowAction::AiReview, RowAction::Approve, RowAction::Bookmark] {
                view.run_row_action(&open, action, cx);
            }
            assert!(view.pending.is_none());
            assert!(view.status.is_some());
            // The PR's page is no list: its rows' menus no longer act.
            view.change_account(Some(account("First")), cx);
            view.navigate(Route::Pull(target(), PullTab::Conversation), cx);
            view.status = None;
            view.run_row_action(&open, RowAction::AiReview, cx);
            assert!(view.status.is_some());
            // Shown again for its own account, it acts.
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            shown(view, vec![data::pull_row(summary("OPEN", false))]);
            view.run_row_action(&open, RowAction::AiReview, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(*handed.borrow(), vec![(account("First"), target())]);
    assert!(writes(&transport).is_empty());
}
