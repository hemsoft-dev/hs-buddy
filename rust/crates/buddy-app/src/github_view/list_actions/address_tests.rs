//! The row menu's Address Unresolved Comments (usePRContextMenu.ts:140).
use super::super::tests::{account, setup};
use super::*;
use crate::copilot_view::chat::AssistantPrompt;
use buddy_core::github::{PullSummary, RepositoryRef};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

fn target() -> PullRef {
    PullRef::new(RepositoryRef::parse("team/private").unwrap(), 7).unwrap()
}

fn summary(threads: u64) -> PullSummary {
    PullSummary {
        target: target(),
        title: "Listed fixture review".into(),
        author: "fixture-author".into(),
        body: None,
        state: "OPEN".into(),
        created_at: "2026-10-15T12:00:00Z".into(),
        updated_at: "2026-10-15T12:00:00Z".into(),
        merged_at: None,
        head_branch: "feature".into(),
        base_branch: "main".into(),
        head_sha: "abcdef0123456789abcdef0123456789abcdef01".into(),
        draft: false,
        i_approved: false,
        approval_count: Some(0),
        review_decision: None,
        thread_count: threads,
        changed_files: 1,
    }
}

fn comment(id: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "body": body, "createdAt": "2026-10-15T12:00:00Z",
        "updatedAt": "2026-10-15T12:00:00Z", "url": null,
        "diffHunk": "@@ -1 +1 @@\n-old\n+new", "author": {"login": "fixture-reviewer"},
        "reactionGroups": []
    })
}

fn thread(id: &str, resolved: bool, body: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "isResolved": resolved, "isOutdated": false, "path": "src/lib.rs",
        "line": 3, "diffSide": "RIGHT",
        "comments": {"pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [comment(&format!("{id}_C"), body)]}
    })
}

/// GitHub's review threads of the PR, or a failure for every read.
struct Threads {
    nodes: Vec<serde_json::Value>,
    fail: bool,
    calls: Mutex<Vec<github::ApiRequest>>,
}

impl Threads {
    fn new(nodes: Vec<serde_json::Value>) -> Arc<Self> {
        Arc::new(Self {
            nodes,
            fail: false,
            calls: Mutex::default(),
        })
    }
}

impl github::Transport for Threads {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        request: github::ApiRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<github::ApiResponse, GitHubError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request.clone());
            if self.fail {
                return Err(GitHubError::Network);
            }
            let query = request.body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(query.contains("NativePullThreads("), "{query}");
            Ok(github::ApiResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: serde_json::json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
                    "pageInfo": {"hasNextPage": false, "endCursor": null},
                    "nodes": self.nodes,
                }}}}}),
            })
        })
    }
}

fn list_view(
    cx: &mut gpui_kit::TestAppContext,
    transport: Arc<Threads>,
    threads: u64,
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
                show(&mut view, threads);
                view
            });
            entity = Some(view.clone());
            view
        })
        .unwrap();
    });
    entity.unwrap()
}

fn show(view: &mut GitHubView, threads: u64) {
    view.data = Some(PageData {
        title: "Needs Review".into(),
        rows: vec![data::pull_row(summary(threads))],
        ..Default::default()
    });
}

fn listed(view: &GitHubView) -> ListedPull {
    let row = &view.data.as_ref().unwrap().rows[0];
    view.listed(row).unwrap().clone()
}

fn entry(view: &GitHubView) -> (String, bool) {
    let entries = view.menu_entries(&listed(view));
    let entry = &entries[2];
    assert_eq!(entry.action, RowAction::AddressComments);
    (entry.label.clone(), entry.enabled)
}

fn sent(
    cx: &mut gpui_kit::TestAppContext,
    view: &Entity<GitHubView>,
) -> Rc<RefCell<Vec<AssistantPrompt>>> {
    let sent = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let sent = sent.clone();
        cx.subscribe(view, move |_, event: &GitHubEvent, _| {
            if let GitHubEvent::SendAssistantPrompt(prompt) = event {
                sent.borrow_mut().push(prompt.clone());
            }
        })
        .detach();
    });
    sent
}

#[gpui_kit::test]
fn address_unresolved_comments_sends_electrons_prompt_with_the_unresolved_threads(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Threads::new(vec![
        thread("PRRT_done", true, "RESOLVEDTEXT already fixed"),
        thread("PRRT_open", false, "Please rename UNRESOLVEDTEXT"),
    ]);
    let view = list_view(cx, transport.clone(), 2);
    let sent = sent(cx, &view);
    cx.update(|cx| {
        // Electron reads copilot.premiumModel as the entry is clicked.
        let mut config = Settings::global(cx).config.clone();
        config.copilot.premium_model = "premium-fixture".into();
        cx.set_global(Settings::new(config));
        view.update(cx, |view, cx| {
            assert_eq!(entry(view), ("Address Unresolved Comments".into(), true));
            let open = view.row_target(&listed(view)).unwrap();
            view.run_row_action(&open, RowAction::AddressComments, cx);
            assert_eq!(
                view.status.as_deref(),
                Some("Reading the unresolved review threads of team/private #7…")
            );
            // One read at a time.
            assert_eq!(entry(view), ("Address Unresolved Comments".into(), false));
        })
    });
    cx.run_until_parked();
    let sent = sent.borrow();
    assert_eq!(sent.len(), 1);
    let prompt = &sent[0];
    assert_eq!(prompt.prompt, address_comments::prompt(&target()));
    assert_eq!(prompt.model.as_deref(), Some("premium-fixture"));
    assert_eq!(
        prompt.context_label.as_deref(),
        Some("1 unresolved review thread")
    );
    let context = prompt.context.as_deref().unwrap();
    assert!(
        context.contains("Please rename UNRESOLVEDTEXT"),
        "{context}"
    );
    assert!(
        !context.contains("RESOLVEDTEXT already"),
        "resolved threads stay out"
    );
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(
            view.status.as_deref(),
            Some("Sent to Copilot Assistant · team/private #7 · 1 unresolved review thread")
        );
        assert_eq!(entry(view), ("Address Unresolved Comments".into(), true));
    });
    // A read only: the assistant, not this view, would change anything.
    assert!(
        transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.is_write)
    );
}

#[gpui_kit::test]
fn a_pr_without_threads_offers_none_and_resolved_threads_send_nothing(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Threads::new(vec![thread("PRRT_done", true, "Done")]);
    let view = list_view(cx, transport.clone(), 0);
    let sent = sent(cx, &view);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            assert_eq!(entry(view), ("No Unresolved Comments".into(), false));
            // A disabled entry does nothing even when reached directly.
            let open = view.row_target(&listed(view)).unwrap();
            view.run_row_action(&open, RowAction::AddressComments, cx);
            assert!(view.status.is_none());
            show(view, 1);
            view.run_row_action(&open, RowAction::AddressComments, cx);
        })
    });
    cx.run_until_parked();
    assert!(sent.borrow().is_empty());
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).status.as_deref(),
            Some("team/private #7 has no unresolved review comments.")
        );
    });
}

#[gpui_kit::test]
fn an_open_menu_reads_the_row_as_the_list_shows_it_and_drops_a_stale_reply(
    cx: &mut gpui_kit::TestAppContext,
) {
    let transport = Threads::new(vec![thread("PRRT_open", false, "Please rename")]);
    let view = list_view(cx, transport.clone(), 0);
    let sent = sent(cx, &view);
    // The menu opened while the row listed no threads.
    let open = cx.update(|cx| {
        let view = view.read(cx);
        view.row_target(&listed(view)).unwrap()
    });
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            // The list was read again since and lists a thread: the entry acts.
            show(view, 1);
            view.run_row_action(&open, RowAction::AddressComments, cx);
            assert!(view.addressing.is_some());
            // Another account's list holds the same PR before the read ends.
            view.change_account(Some(account("Second")), cx);
            view.navigate(Route::Pulls(PrMode::NeedsReview), cx);
            show(view, 1);
        })
    });
    cx.run_until_parked();
    assert!(
        sent.borrow().is_empty(),
        "never sent for another account's list"
    );
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            assert_eq!(
                view.status.as_deref(),
                Some("This pull request is no longer in the shown list.")
            );
            assert!(view.addressing.is_none());
            // The first account's menu, still open, is refused on this list.
            view.status = None;
            view.run_row_action(&open, RowAction::AddressComments, cx);
            assert_eq!(
                view.status.as_deref(),
                Some("This pull request is no longer in the shown list.")
            );
            assert!(view.addressing.is_none());
        })
    });
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
fn a_failed_thread_read_is_shown_and_sends_nothing(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(Threads {
        nodes: Vec::new(),
        fail: true,
        calls: Mutex::default(),
    });
    let view = list_view(cx, transport, 1);
    let sent = sent(cx, &view);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let open = view.row_target(&listed(view)).unwrap();
            view.run_row_action(&open, RowAction::AddressComments, cx);
        })
    });
    cx.run_until_parked();
    assert!(sent.borrow().is_empty());
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(
            view.status.as_deref(),
            Some(GitHubError::Network.to_string().as_str())
        );
        assert_eq!(entry(view), ("Address Unresolved Comments".into(), true));
    });
}
