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
            view.run_row_action(&pull, RowAction::Approve, cx);
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
            let pull = listed(view);
            view.run_row_action(&pull, RowAction::Approve, cx);
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
            view.run_row_action(&pull, RowAction::Approve, cx);
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
            let pull = listed(view);
            view.run_row_action(&pull, RowAction::RequestCopilot, cx);
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
            view.run_row_action(&merged, RowAction::Approve, cx);
            assert!(view.pending.is_none());

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
            let pull = listed(view);
            view.run_row_action(&pull, RowAction::CopyLink, cx);
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
            let pull = listed(view);
            view.run_row_action(&pull, RowAction::RequestCopilot, cx);
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
            let pull = listed(view);
            view.run_row_action(&pull, RowAction::Approve, cx);
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
