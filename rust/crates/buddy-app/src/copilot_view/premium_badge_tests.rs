//! Electron's `PremiumUsageBadge` beside the prompt box's account picker.
use super::premium_badge::{Quota, State, TTL};
use super::*;
use buddy_core::config::GitHubAccount;
use buddy_core::copilot_usage::{PremiumQuota, parse_personal_quota, parse_premium_quota};
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

/// GitHub's figures: remaining, entitlement and `percent_remaining`.
fn premium(remaining: f64, entitlement: f64, percent_remaining: f64) -> PremiumQuota {
    PremiumQuota {
        entitlement,
        remaining,
        percent_remaining,
    }
}

fn quota(remaining: f64, entitlement: f64, percent_remaining: f64) -> Quota {
    Quota::from_premium(&premium(remaining, entitlement, percent_remaining))
}

/// A `/copilot_internal/user` body with this `premium_interactions` object.
fn snapshot(premium: &str) -> String {
    format!(r#"{{"quota_snapshots":{{"premium_interactions":{premium}}}}}"#)
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
        config.github.accounts = vec![
            account("alice"),
            account("bob"),
            account("carol"),
            account("dave"),
        ];
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
                    Ok(premium(174.0, 300.0, 58.0))
                } else if account.username == "dave" {
                    // A successful response that names no figures.
                    parse_premium_quota(&snapshot("{}"))
                } else {
                    Ok(premium(50.0, 1000.0, 5.0))
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
    let shown = quota(174.0, 300.0, 58.0);
    assert_eq!(
        shown,
        Quota {
            remaining: 174.0,
            entitlement: 300.0,
            percent_used: 42.0
        }
    );
    assert_eq!(shown.text(), "42% · 174/300");
    assert_eq!(
        shown.tooltip(),
        "Premium: 174 of 300 remaining (42.0% used)"
    );
    // toFixed rounds a half up.
    let half = quota(7.0, 8.0, 87.5);
    assert_eq!(half.text(), "13% · 7/8");
    assert_eq!(half.tooltip(), "Premium: 7 of 8 remaining (12.5% used)");
    // Overage shows more than 100%.
    let over = Quota::from_premium(&PremiumQuota {
        entitlement: 300.0,
        remaining: -30.0,
        percent_remaining: -10.0,
    });
    assert_eq!(over.text(), "110% · -30/300");
    // Electron shows an unlimited quota's 100 percent remaining as 0% used.
    let unlimited = Quota::from_premium(&PremiumQuota {
        entitlement: 0.0,
        remaining: 0.0,
        percent_remaining: 100.0,
    });
    assert_eq!(unlimited.text(), "0% · 0/0");
    assert_eq!(unlimited.color(), 0x4ec9b0);
}

#[test]
fn the_percentage_and_colour_come_from_githubs_unrounded_figures() {
    // 10.4 remaining of 100 is 89.6% used: orange, and shown as 10.4, where
    // the rounded counts would say 10 remaining and red.
    let fractional = Quota::from_premium(&PremiumQuota {
        entitlement: 100.0,
        remaining: 10.4,
        percent_remaining: 10.4,
    });
    assert_eq!(fractional.text(), "90% · 10.4/100");
    assert_eq!(
        fractional.tooltip(),
        "Premium: 10.4 of 100 remaining (89.6% used)"
    );
    assert_eq!(fractional.color(), 0xe89b3c);
    // getQuotaColor's thresholds, on either side of each edge.
    let colour = |percent_remaining: f64| {
        Quota::from_premium(&PremiumQuota {
            entitlement: 100.0,
            remaining: percent_remaining,
            percent_remaining,
        })
        .color()
    };
    assert_eq!(
        [
            colour(50.1),
            colour(50.0),
            colour(25.1),
            colour(25.0),
            colour(10.1),
            colour(10.0)
        ],
        [0x4ec9b0, 0xdcd34a, 0xdcd34a, 0xe89b3c, 0xe89b3c, 0xe85d5d]
    );
}

#[test]
fn an_incomplete_quota_response_is_no_reading() {
    let now = jiff::Timestamp::UNIX_EPOCH;
    // Neither the badge nor the Usage pane reads a missing figure as zero.
    for premium in [
        "{}",
        r#"{"remaining":10,"percent_remaining":10}"#,
        r#"{"entitlement":100,"percent_remaining":10}"#,
        r#"{"entitlement":null,"remaining":10,"percent_remaining":10}"#,
    ] {
        let json = snapshot(premium);
        assert!(parse_premium_quota(&json).is_err(), "{premium}");
        assert!(parse_personal_quota(&json, now).is_err(), "{premium}");
    }
    // The badge needs the percentage Electron subtracts from 100.
    for premium in [
        r#"{"entitlement":100,"remaining":10}"#,
        r#"{"entitlement":100,"remaining":10,"percent_remaining":null}"#,
    ] {
        assert!(
            parse_premium_quota(&snapshot(premium)).is_err(),
            "{premium}"
        );
    }
    for json in [
        "{}",
        r#"{"quota_snapshots":{}}"#,
        r#"{"quota_snapshots":{"premium_interactions":null}}"#,
    ] {
        assert!(parse_premium_quota(json).is_err(), "{json}");
    }
    // A complete reading, and an unlimited one, as GitHub sends them.
    assert_eq!(
        parse_premium_quota(&snapshot(
            r#"{"entitlement":300,"remaining":174,"percent_remaining":58,"overage_count":0}"#
        )),
        Ok(PremiumQuota {
            entitlement: 300.0,
            remaining: 174.0,
            percent_remaining: 58.0
        })
    );
    assert_eq!(
        parse_premium_quota(&snapshot(
            r#"{"entitlement":0,"remaining":0,"percent_remaining":100,"unlimited":true}"#
        ))
        .map(|premium| Quota::from_premium(&premium).text()),
        Ok("0% · 0/0".to_string())
    );
}

#[gpui_kit::test]
fn a_configured_account_shows_its_quota_and_switching_back_uses_the_cache(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view, calls) = open(cx, &[]);
    assert_eq!(state(cx, &view), State::Hidden);
    assert_eq!(shown(cx, handle, &view), None);
    choose(cx, handle, &view, "alice");
    assert_eq!(state(cx, &view), State::Shown(quota(174.0, 300.0, 58.0)));
    assert_eq!(shown(cx, handle, &view).as_deref(), Some("42% · 174/300"));
    // Another account reads its own quota, and returning within five minutes
    // reads nothing. Logins match case-insensitively.
    choose(cx, handle, &view, "bob");
    assert_eq!(state(cx, &view), State::Shown(quota(50.0, 1000.0, 5.0)));
    choose(cx, handle, &view, "Alice");
    assert_eq!(state(cx, &view), State::Shown(quota(174.0, 300.0, 58.0)));
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
            .store("alice", quota(299.0, 300.0, 99.7), old);
        assert!(
            cx.global::<QuotaCache>()
                .fresh("alice", Instant::now())
                .is_none()
        );
        let fresh = Instant::now()
            .checked_sub(TTL - Duration::from_secs(5))
            .unwrap();
        cx.default_global::<QuotaCache>()
            .store("bob", quota(299.0, 300.0, 99.7), fresh);
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
    assert_eq!(state(cx, &view), State::Shown(quota(174.0, 300.0, 58.0)));
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
    // A successful response without the figures is no reading either: the
    // badge shows nothing and nothing is kept for five minutes.
    choose(cx, handle, &view, "dave");
    assert_eq!(state(cx, &view), State::Failed);
    assert_eq!(shown(cx, handle, &view), None);
    // Alice's reading is still cached; Dave's answer was not.
    choose(cx, handle, &view, "alice");
    choose(cx, handle, &view, "dave");
    assert_eq!(*calls.borrow(), ["carol", "alice", "carol", "dave", "dave"]);
    cx.update(|cx| {
        assert!(
            cx.global::<QuotaCache>()
                .fresh("dave", Instant::now())
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
            view.badge_fetched(
                earlier,
                Some(Ok(premium(1.0, 2.0, 50.0))),
                Instant::now(),
                cx,
            )
        })
    });
    assert_eq!(state(cx, &view), State::Shown(quota(50.0, 1000.0, 5.0)));
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
