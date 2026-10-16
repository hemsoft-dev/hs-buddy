use super::*;
use buddy_core::github::{PullRef, RepositoryRef};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct WriteFixture(Mutex<Vec<github::ApiRequest>>);
impl github::Transport for WriteFixture {
    fn request<'a>(
        &'a self,
        _: &'a AccountScope,
        request: github::ApiRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<github::ApiResponse, GitHubError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let write = request.is_write;
            self.0.lock().unwrap().push(request);
            Ok(github::ApiResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: if write {
                    serde_json::json!({"id":7})
                } else {
                    serde_json::json!({"data":{"repository":{"pullRequest":{
                        "comments":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}},
                        "reviews":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}
                    }}}})
                },
            })
        })
    }
}

fn write_fixture_view(
    cx: &mut gpui_kit::TestAppContext,
    transport: Arc<WriteFixture>,
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
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                view
            });
            entity = Some(view.clone());
            view
        })
        .unwrap();
    });
    entity.unwrap()
}

fn wait_for_write(cx: &mut gpui_kit::TestAppContext, view: &Entity<GitHubView>) {
    cx.run_until_parked();
    assert!(
        cx.update(|cx| !view.read(cx).write_busy),
        "The fixture write did not finish"
    );
}

#[gpui_kit::test]
fn confirmed_comment_does_not_erase_a_newer_unsubmitted_draft(cx: &mut gpui_kit::TestAppContext) {
    let transport = Arc::new(WriteFixture::default());
    let view = write_fixture_view(cx, transport.clone());
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.prepare(
                WriteAction::Comment {
                    pull: pull(),
                    body: "Reviewed body".into(),
                },
                cx,
            )
        })
    });
    // The editor remains editable while the immutable confirmation is open.
    // Use its real window so GPUI performs the normal deferred draft clearing.
    cx.update(|cx| {
        let window = cx.windows()[0];
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    drop(view.render(window, cx));
                    view.draft.update(cx, |input, cx| {
                        input.set_value("New unsent draft", window, cx)
                    });
                    view.confirm(cx);
                })
            })
            .unwrap();
    });
    wait_for_write(cx, &view);
    cx.update(|cx| {
        let window = cx.windows()[0];
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    drop(view.render(window, cx));
                    assert_eq!(view.draft.read(cx).value().to_string(), "New unsent draft");
                })
            })
            .unwrap();
    });
    let calls = transport.0.lock().unwrap();
    let sent: Vec<_> = calls.iter().filter(|call| call.is_write).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body.as_ref().unwrap()["body"], "Reviewed body");
}

#[gpui_kit::test]
fn two_bot_requests_keep_independent_watches_for_the_same_pr(cx: &mut gpui_kit::TestAppContext) {
    let view = write_fixture_view(cx, Arc::new(WriteFixture::default()));
    for action in [
        WriteAction::RequestCodeRabbit { pull: pull() },
        WriteAction::RequestCopilot { pull: pull() },
    ] {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                if view.must_reload {
                    let context = LoadContext {
                        ticket: view.selection.begin().unwrap(),
                        force: true,
                        more: false,
                        write_key: view.current_write_key(),
                        can_unlock: true,
                    };
                    view.receive_load(context, Ok(PageData::default()));
                }
                view.prepare(action, cx);
                assert!(view.pending.is_some());
                view.confirm(cx);
            })
        });
        wait_for_write(cx, &view);
    }
    cx.update(|cx| {
        view.update(cx, |view, _| {
            assert_eq!(view.review_watches.len(), 2);
            assert!(
                view.review_watches
                    .values()
                    .any(|watch| watch.kind == github::ReviewKind::CodeRabbit)
            );
            assert!(
                view.review_watches
                    .values()
                    .any(|watch| watch.kind == github::ReviewKind::Copilot)
            );
            let rabbit = view
                .review_watches
                .keys()
                .find(|(_, kind)| *kind == github::ReviewKind::CodeRabbit)
                .unwrap()
                .clone();
            view.record_review_result(
                rabbit.clone(),
                Ok(github::ReviewProgress::Completed { url: None }),
            );
            assert_eq!(view.review_watches.len(), 1);
            assert_eq!(
                view.review_watches.values().next().unwrap().kind,
                github::ReviewKind::Copilot
            );
            assert!(view.review_status[&rabbit].contains("review is available"));
            let copilot = view.review_watches.keys().next().unwrap().clone();
            assert!(view.review_status[&copilot].contains("review requested"));
            view.record_review_result(
                copilot.clone(),
                Ok(github::ReviewProgress::Completed { url: None }),
            );
            assert!(view.review_watches.is_empty());
            assert!(view.review_status[&copilot].contains("review is available"));
        })
    });
}

fn pull() -> PullRef {
    PullRef::new(RepositoryRef::parse("team/private").unwrap(), 7).unwrap()
}
fn account(user: &str) -> AccountScope {
    AccountScope::new(user, "team").unwrap()
}
fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    let mut config = buddy_core::config::AppConfig::default();
    config.github.accounts = ["First", "Second"]
        .into_iter()
        .map(|username| buddy_core::config::GitHubAccount {
            username: username.into(),
            org: "team".into(),
            repo_root: None,
            usage_provider: None,
            extra: BTreeMap::new(),
        })
        .collect();
    crate::theme::install(&config, cx);
    cx.set_global(Settings::new(config));
}

#[gpui_kit::test]
fn changing_the_merged_window_invalidates_the_old_list_and_inflight_result(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        setup(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pulls(PrMode::RecentlyMerged), cx);
                view.data = Some(PageData {
                    title: "Old seven-day list".into(),
                    ..Default::default()
                });
                let old = view.selection.begin().unwrap();
                let mut updated = Settings::global(cx).config.clone();
                updated.pr.recently_merged_days = 1.0;
                Settings::replace(cx, updated);
                view.reconcile(cx);
                assert!(
                    view.data.is_none(),
                    "the old date window must not remain visible after a Settings edit"
                );
                assert!(!view.selection.accepts(&old));
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn drafts_survive_appearance_updates_and_clear_with_the_account_and_confirmation(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        setup(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                drop(view.render(window, cx));
                view.draft.update(cx, |input, cx| {
                    input.set_value("Private\nmultiline draft", window, cx)
                });
                let mut appearance = Settings::global(cx).config.clone();
                appearance.ui.font_family = "Fixture font".into();
                Settings::replace(cx, appearance);
                view.reconcile(cx);
                drop(view.render(window, cx));
                assert_eq!(
                    view.draft.read(cx).value().to_string(),
                    "Private\nmultiline draft"
                );
                view.prepare(
                    WriteAction::Comment {
                        pull: pull(),
                        body: "Private draft".into(),
                    },
                    cx,
                );
                assert!(view.pending.is_some());
                view.data = Some(PageData {
                    title: "First account private data".into(),
                    ..Default::default()
                });
                view.change_account(Some(account("Second")), cx);
                assert!(view.pending.is_none());
                assert!(view.data.is_none());
                drop(view.render(window, cx));
                assert!(view.draft.read(cx).value().is_empty());
                assert!(view.nudge_email.read(cx).value().is_empty());
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn stale_results_and_failed_reload_cannot_replace_another_account_or_unlock_an_unknown_write(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        setup(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = GitHubView::new(window, cx);
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                let stale = view.selection.begin().unwrap();
                let key = view.current_write_key().unwrap();
                view.write_locks.insert(key.clone(), view.route.clone());
                view.update_write_guard();
                assert!(view.must_reload);
                view.change_account(Some(account("Second")), cx);
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                view.receive_load(
                    LoadContext {
                        ticket: stale,
                        force: true,
                        more: false,
                        write_key: Some(key.clone()),
                        can_unlock: true,
                    },
                    Ok(PageData {
                        title: "Leaked First data".into(),
                        ..Default::default()
                    }),
                );
                assert!(view.data.is_none());
                assert!(view.write_locks.contains_key(&key));
                view.change_account(Some(account("First")), cx);
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                assert!(view.must_reload);
                view.status = Some("Comment posted; refresh to read it.".into());
                let request = LoadContext {
                    ticket: view.selection.begin().unwrap(),
                    force: true,
                    more: false,
                    write_key: Some(key.clone()),
                    can_unlock: true,
                };
                view.receive_load(request, Err(GitHubError::Timeout));
                assert!(view.must_reload);
                assert_eq!(
                    view.status.as_deref(),
                    Some("Comment posted; refresh to read it.")
                );
                assert!(view.error.is_some());
                let request = LoadContext {
                    ticket: view.selection.begin().unwrap(),
                    force: true,
                    more: false,
                    write_key: Some(key.clone()),
                    can_unlock: false,
                };
                view.receive_load(request, Ok(PageData::default()));
                assert!(
                    view.must_reload,
                    "a read begun during the write cannot unlock it"
                );
                view.navigate(Route::Pull(pull(), PullTab::Files), cx);
                let request = LoadContext {
                    ticket: view.selection.begin().unwrap(),
                    force: true,
                    more: false,
                    write_key: Some(key.clone()),
                    can_unlock: true,
                };
                view.receive_load(request, Ok(PageData::default()));
                assert!(
                    view.must_reload,
                    "refreshing file diffs cannot verify whether the uncertain comment was posted"
                );
                view.navigate(Route::Pull(pull(), PullTab::Conversation), cx);
                let request = LoadContext {
                    ticket: view.selection.begin().unwrap(),
                    force: true,
                    more: false,
                    write_key: Some(key),
                    can_unlock: true,
                };
                view.receive_load(request, Ok(PageData::default()));
                assert!(!view.must_reload);
                view
            })
        })
        .unwrap();
    });
}
