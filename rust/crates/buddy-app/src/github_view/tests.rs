use super::*;
use buddy_core::github::{PullRef, RepositoryRef};

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
