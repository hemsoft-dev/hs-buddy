use super::*;
use gpui_kit::test::TestWindowExt as _;

fn setup(cx: &mut gpui_kit::TestAppContext) -> Entity<ResultsView> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| ResultsView::new(window, cx))
        })
        .unwrap();
        view
    })
}
fn dropped() -> futures::channel::oneshot::Receiver<()> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    drop(sender);
    receiver
}

#[gpui_kit::test]
fn a_panicked_worker_channel_unlocks_each_operation_and_preserves_unconfirmed_deletion(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    for kind in 0..3 {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.list_loading = kind == 0;
                view.detail_loading = kind == 1;
                view.write_loading = kind == 2;
                view.list_error = None;
                view.detail_error = None;
                view.selected_id = Some("owned-result".into());
                let loading = match kind {
                    0 => Loading::List(view.list_ticket),
                    1 => Loading::Detail(view.detail_ticket),
                    _ => Loading::Delete,
                };
                view.receive(cx, loading, dropped(), |_, _, _| {
                    panic!("A dropped worker cannot complete successfully")
                });
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(!view.busy());
            assert!(view.selected_id.as_deref() == Some("owned-result"));
            if kind == 0 {
                assert!(view.list_error.is_some());
            } else {
                assert!(view.detail_error.is_some());
            }
            if kind == 2 {
                assert!(view.detail_error.as_ref().unwrap().contains("unconfirmed"));
                assert!(!view.delete_confirm);
            }
        });
    }
}

#[gpui_kit::test]
fn an_old_dropped_worker_cannot_unlock_a_new_request(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    for detail in [false, true] {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.list_loading = !detail;
                view.detail_loading = detail;
                view.list_error = None;
                view.detail_error = None;
                let old = uuid::Uuid::new_v4();
                let loading = if detail {
                    Loading::Detail(old)
                } else {
                    Loading::List(old)
                };
                view.receive(cx, loading, dropped(), |_, _, _| {
                    panic!("A dropped worker cannot complete successfully")
                });
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.busy() && view.list_error.is_none() && view.detail_error.is_none());
        });
    }
}

const PR: &str = "https://github.com/owner/repo/pull/7";

fn summaries(rows: serde_json::Value) -> Vec<ResultSummary> {
    let page = serde_json::json!({"page": rows, "isDone": true, "continueCursor": ""});
    buddy_core::copilot_results::summaries::parse_results(&serde_json::to_vec(&page).unwrap())
        .ok()
        .unwrap()
        .rows
}

fn saved(category: &str) -> SavedResult {
    let record = serde_json::json!({"_id": "review-result", "prompt": "Review prompt",
        "status": "completed", "result": "Answer", "category": category,
        "metadata": {"prUrl": PR, "owner": "owner"}, "createdAt": 1});
    buddy_core::copilot_results::parse_detail(&serde_json::to_vec(&record).unwrap())
        .ok()
        .flatten()
        .unwrap()
}

fn window(cx: &mut gpui_kit::TestAppContext) -> (Entity<ResultsView>, gpui_kit::VisualTestContext) {
    let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        cx.set_global(crate::settings::Settings::new(
            buddy_core::config::AppConfig::default(),
        ));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| ResultsView::new(window, cx))
        })
        .unwrap()
    });
    let visual = gpui_kit::VisualTestContext::from_window(handle, cx);
    (view, visual)
}

#[gpui_kit::test]
fn a_pr_review_row_opens_its_pull_request_without_opening_the_result(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (view, mut visual) = window(cx);
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.initialized = true;
            view.rows = summaries(serde_json::json!([
                {"_id": "general", "promptPreview": "General", "status": "completed",
                    "categoryLabel": "general", "prUrl": PR, "createdAt": 2, "hasOutput": true},
                {"_id": "review", "promptPreview": "Review", "status": "completed",
                    "categoryLabel": "pr-review", "prUrl": PR, "createdAt": 1, "hasOutput": true},
            ]));
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("copilot-saved-row-open-pr", 0usize))
                .is_none()
        );
        window.click(("copilot-saved-row-open-pr", 1usize), cx);
    });
    assert_eq!(visual.opened_url().as_deref(), Some(PR));
    visual.update(|_, cx| {
        let view = view.read(cx);
        assert!(view.selected_id.is_none() && !view.detail_loading);
    });
}

#[gpui_kit::test]
fn the_detail_header_opens_the_reviewed_pull_request_only_for_a_pr_review(
    cx: &mut gpui_kit::TestAppContext,
) {
    // The test platform keeps the last opened URL, so the absent case runs first.
    for (category, offered) in [("general", false), ("pr-review", true)] {
        let (view, mut visual) = window(cx);
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.selected_id = Some("review-result".into());
                view.selected = Some(saved(category));
                cx.notify();
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.try_find("copilot-saved-open-pr").is_some(), offered);
            if offered {
                window.click("copilot-saved-open-pr", cx);
            }
        });
        assert_eq!(visual.opened_url().as_deref(), offered.then_some(PR));
    }
}
