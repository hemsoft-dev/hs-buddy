use super::*;

fn setup(cx: &mut gpui_kit::TestAppContext) -> Entity<UsageView> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| UsageView::new(window, cx))
        })
        .unwrap();
        view
    })
}
fn pool(used: i64) -> UsagePool {
    UsagePool {
        used,
        allotment: 100,
        overage_count: 0,
        reset_at: "2026-11-01T00:00:00Z".parse().unwrap(),
    }
}
fn seed(view: &mut UsageView) {
    view.pool = Some(pool(10));
    view.billing = Some(BillingSnapshot {
        usage: Default::default(),
        budget_amount: Some(100.0),
        budget_unavailable: false,
        prevent_further_usage: false,
        fetched_at: Utc::now(),
        year: 2026,
        month: 10,
    });
    view.codex = Some(UsageSnapshot {
        plan_type: Some("synthetic".into()),
        windows: Vec::new(),
        fetched_at: 1,
    });
    view.history = Some(Page {
        page: Vec::new(),
        continue_cursor: "owned-cursor".into(),
        is_done: true,
        page_status: None,
    });
    view.history_period = Some((2026, 9));
    view.busy = true;
}

#[gpui_kit::test]
fn failed_provider_refreshes_remove_old_live_values_without_erasing_selected_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            seed(view);
            view.finish_refresh(Ok((
                None,
                Some(Err("unavailable".into())),
                Some(Err("billing unavailable".into())),
                None,
            )));
            assert!(
                view.pool.is_none() && view.billing.is_none() && view.error.is_some() && !view.busy
            );
            assert_eq!(view.history_period, Some((2026, 9)));
            assert!(view.history.is_some());
            view.finish_refresh(Ok((
                Some(Err(codex_usage::FetchError::Unavailable)),
                None,
                None,
                None,
            )));
            assert!(view.codex.is_none() && view.error.is_some());
        })
    });
}

#[gpui_kit::test]
fn a_partial_refresh_only_exposes_the_successful_new_value(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            seed(view);
            view.finish_refresh(Ok((
                None,
                Some(Ok(pool(25))),
                Some(Err("billing unavailable".into())),
                None,
            )));
            assert_eq!(view.pool.as_ref().unwrap().used, 25);
            assert!(view.billing.is_none() && view.error.is_some());
            assert_eq!(view.history_period, Some((2026, 9)));
        })
    });
}

#[gpui_kit::test]
fn a_dropped_usage_worker_removes_live_values_and_unlocks_refresh(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    let (sender, receiver) = futures::channel::oneshot::channel::<RefreshReply>();
    drop(sender);
    let result = futures::executor::block_on(receiver);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            seed(view);
            view.finish_refresh(result);
            assert!(view.pool.is_none() && view.billing.is_none() && view.codex.is_none());
            assert!(!view.busy && view.error.is_some() && view.history.is_some());
        })
    });
}
