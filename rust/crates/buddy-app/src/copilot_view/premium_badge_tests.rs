//! Electron's `PremiumUsageBadge` beside the prompt box's account picker.
use super::premium_badge::{Quota, State, TTL};
use super::*;
use buddy_core::config::GitHubAccount;
use buddy_core::copilot_usage::UsagePool;
use gpui_kit::test::TestWindowExt as _;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn account(username: &str) -> GitHubAccount {
    GitHubAccount {
        username: username.into(),
        org: "fixture-org".into(),
        repo_root: None,
        usage_provider: None,
        extra: Default::default(),
    }
}

fn pool(used: i64, allotment: i64) -> UsagePool {
    UsagePool {
        used,
        allotment,
        overage_count: 0,
        reset_at: jiff::Timestamp::UNIX_EPOCH,
    }
}

type Calls = Rc<RefCell<Vec<String>>>;

/// Configured accounts and a fake quota read that records each account it is
/// asked for; `failing` logins answer with an error.
fn open(
    cx: &mut gpui_kit::TestAppContext,
    failing: &'static [&'static str],
) -> (gpui_kit::AnyWindowHandle, Entity<ChatView>, Calls) {
    let calls: Calls = Rc::default();
    let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.github.accounts = vec![account("alice"), account("bob"), account("carol")];
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| ChatView::new(window, cx))
        })
        .unwrap()
    });
    let recorded = calls.clone();
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.badge.fetch = Rc::new(move |account: GitHubAccount| {
                recorded.borrow_mut().push(account.username.clone());
                let result = if failing.contains(&account.username.as_str()) {
                    Err("Sign in with GitHub CLI for the selected account.".to_string())
                } else if account.username == "alice" {
                    Ok(pool(126, 300))
                } else {
                    Ok(pool(950, 1000))
                };
                Box::pin(futures::future::ready(result))
            });
        })
    });
    (handle, view, calls)
}

fn choose(
    cx: &mut gpui_kit::TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    view: &Entity<ChatView>,
    login: &str,
) {
    let input = cx.update(|cx| view.read(cx).account.clone());
    cx.update_window(handle, |_, window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(login.to_string(), window, cx)
        });
        // The render follows the account field, as Electron's effect does.
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn state(cx: &mut gpui_kit::TestAppContext, view: &Entity<ChatView>) -> State {
    cx.update(|cx| view.read(cx).badge.state)
}

/// What the rendered badge shows, after a frame.
fn shown(
    cx: &mut gpui_kit::TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    view: &Entity<ChatView>,
) -> Option<String> {
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.update(|cx| view.read(cx).shown_quota().map(|quota| quota.text()))
}

#[test]
fn the_badge_shows_electrons_percentage_figures_and_colours() {
    let quota = Quota::from_pool(&pool(126, 300));
    assert_eq!(
        quota,
        Quota {
            remaining: 174,
            entitlement: 300,
            percent_used: 42.0
        }
    );
    assert_eq!(quota.text(), "42% · 174/300");
    assert_eq!(
        quota.tooltip(),
        "Premium: 174 of 300 remaining (42.0% used)"
    );
    // toFixed rounds a half up.
    let half = Quota::from_pool(&pool(1, 8));
    assert_eq!(half.text(), "13% · 7/8");
    assert_eq!(half.tooltip(), "Premium: 7 of 8 remaining (12.5% used)");
    // getQuotaColor's thresholds.
    let colour = |used| Quota::from_pool(&pool(used, 100)).color();
    assert_eq!(
        [colour(49), colour(50), colour(75), colour(90)],
        [0x4ec9b0, 0xdcd34a, 0xe89b3c, 0xe85d5d]
    );
    // Overage shows more than 100%; an unlimited quota shows 0% used.
    assert_eq!(Quota::from_pool(&pool(330, 300)).text(), "110% · -30/300");
    assert_eq!(Quota::from_pool(&pool(0, 0)).text(), "0% · 0/0");
}

#[gpui_kit::test]
fn a_configured_account_shows_its_quota_and_switching_back_uses_the_cache(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view, calls) = open(cx, &[]);
    assert_eq!(state(cx, &view), State::Hidden);
    assert_eq!(shown(cx, handle, &view), None);
    choose(cx, handle, &view, "alice");
    assert_eq!(
        state(cx, &view),
        State::Shown(Quota::from_pool(&pool(126, 300)))
    );
    assert_eq!(shown(cx, handle, &view).as_deref(), Some("42% · 174/300"));
    // Another account reads its own quota, and returning within five minutes
    // reads nothing. Logins match case-insensitively.
    choose(cx, handle, &view, "bob");
    assert_eq!(
        state(cx, &view),
        State::Shown(Quota::from_pool(&pool(950, 1000)))
    );
    choose(cx, handle, &view, "Alice");
    assert_eq!(
        state(cx, &view),
        State::Shown(Quota::from_pool(&pool(126, 300)))
    );
    // Showing Chat again within five minutes also reuses the cache.
    cx.update(|cx| view.update(cx, |view, cx| view.sync_badge(true, cx)));
    cx.run_until_parked();
    assert_eq!(*calls.borrow(), ["alice", "bob"]);
    // A login that is not a configured account has no quota and reads nothing.
    choose(cx, handle, &view, "stranger");
    assert_eq!(state(cx, &view), State::Hidden);
    assert_eq!(shown(cx, handle, &view), None);
    assert_eq!(calls.borrow().len(), 2);
}

#[gpui_kit::test]
fn a_quota_older_than_five_minutes_is_read_again(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view, calls) = open(cx, &[]);
    choose(cx, handle, &view, "alice");
    // Age the cached quota past the five-minute window.
    cx.update(|cx| {
        let old = Instant::now()
            .checked_sub(TTL + Duration::from_secs(1))
            .unwrap();
        cx.default_global::<QuotaCache>()
            .store("alice", Quota::from_pool(&pool(1, 300)), old);
        assert!(
            cx.global::<QuotaCache>()
                .fresh("alice", Instant::now())
                .is_none()
        );
        let fresh = Instant::now()
            .checked_sub(TTL - Duration::from_secs(5))
            .unwrap();
        cx.default_global::<QuotaCache>()
            .store("bob", Quota::from_pool(&pool(1, 300)), fresh);
        assert!(
            cx.global::<QuotaCache>()
                .fresh("BOB", Instant::now())
                .is_some()
        );
    });
    cx.update(|cx| view.update(cx, |view, cx| view.sync_badge(true, cx)));
    // Electron hides the badge while it reads.
    assert_eq!(state(cx, &view), State::Loading);
    assert_eq!(shown(cx, handle, &view), None);
    cx.run_until_parked();
    assert_eq!(*calls.borrow(), ["alice", "alice"]);
    assert_eq!(
        state(cx, &view),
        State::Shown(Quota::from_pool(&pool(126, 300)))
    );
}

#[gpui_kit::test]
fn a_failed_read_hides_the_badge_and_never_shows_zero(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view, calls) = open(cx, &["carol"]);
    choose(cx, handle, &view, "carol");
    assert_eq!(state(cx, &view), State::Failed);
    assert_eq!(shown(cx, handle, &view), None);
    // A failure is not cached: switching back reads again.
    choose(cx, handle, &view, "alice");
    choose(cx, handle, &view, "carol");
    assert_eq!(*calls.borrow(), ["carol", "alice", "carol"]);
    assert_eq!(state(cx, &view), State::Failed);
    cx.update(|cx| {
        assert!(
            cx.global::<QuotaCache>()
                .fresh("carol", Instant::now())
                .is_none()
        )
    });
}

#[gpui_kit::test]
fn a_late_answer_for_the_previous_account_is_dropped(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view, _) = open(cx, &[]);
    choose(cx, handle, &view, "alice");
    let earlier = cx.update(|cx| view.read(cx).badge.generation);
    choose(cx, handle, &view, "bob");
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.badge_fetched(earlier, Some(Ok(pool(1, 2))), Instant::now(), cx)
        })
    });
    assert_eq!(
        state(cx, &view),
        State::Shown(Quota::from_pool(&pool(950, 1000)))
    );
    // The sidebar assistant has no badge and reads nothing.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.sidebar_context = Some(crate::assistant_context::AssistantContext::section(None));
            view.badge.login.clear();
            view.badge.state = State::Hidden;
            view.sync_badge(true, cx);
            assert_eq!(view.badge.state, State::Hidden);
        })
    });
}
