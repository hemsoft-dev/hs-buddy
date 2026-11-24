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

fn saved_result(status: &str, metadata: serde_json::Value, model: Option<&str>) -> SavedResult {
    let mut value = serde_json::json!({"_id": "saved-result", "prompt": "Saved prompt 界",
        "status": status, "category": "code-analysis", "metadata": metadata,
        "createdAt": 1_792_152_000_000.0});
    if let Some(model) = model {
        value["model"] = model.into();
    }
    buddy_core::copilot_results::parse_detail(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .unwrap()
}

#[test]
fn a_re_run_uses_the_saved_account_and_model_then_falls_back_to_settings() {
    let saved = saved_result(
        "completed",
        serde_json::json!({"ghAccount": "saved-account"}),
        Some("saved-model"),
    );
    let context = rerun::test_identity(&saved, None).unwrap();
    assert_eq!(
        (context.account.as_str(), context.model.as_str()),
        ("saved-account", "saved-model")
    );
    assert!(context.review.is_none());
    let bare = saved_result("failed", serde_json::json!({}), None);
    assert!(rerun::test_identity(&bare, None).is_err());
    let mut config = buddy_core::config::AppConfig::default();
    config.copilot.gh_account = "settings-account".into();
    config.copilot.model = "settings-model".into();
    let context = rerun::test_identity(&bare, Some(&config)).unwrap();
    assert_eq!(
        (context.account.as_str(), context.model.as_str()),
        ("settings-account", "settings-model")
    );
    config.copilot.gh_account = " ".into();
    assert!(rerun::test_identity(&bare, Some(&config)).is_err());
}

/// A real handle whose driver never runs. Nothing is started or written.
fn idle_run(
    temp: &tempfile::TempDir,
) -> Result<
    (
        buddy_core::copilot_assistant::RunHandle,
        std::future::Ready<()>,
    ),
    buddy_core::copilot_runs::Failure,
> {
    buddy_core::copilot_assistant::start(buddy_core::copilot_assistant::RunOptions {
        context: buddy_core::copilot_runs::Context {
            account: "synthetic-account".into(),
            model: "gpt-4.1".into(),
            review: None,
        },
        prompt: "Synthetic test".into(),
        saved: false,
        review: None,
        backend: None,
        executable: temp.path().join("never-start.exe"),
        owned_directory: temp.path().join("must-remain-absent"),
    })
    .map(|(run, _driver)| (run, std::future::ready(())))
}

#[gpui_kit::test]
fn re_run_and_retry_start_one_saved_run_of_the_same_prompt_bound_to_the_saved_result(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    let window = cx.update(|cx| cx.windows()[0]);
    let shown = |cx: &mut gpui_kit::TestAppContext, id: &'static str| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    };
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.selected_id = Some("saved-result".into());
            view.selected = Some(saved_result(
                "completed",
                serde_json::json!({"ghAccount": "saved-account"}),
                Some("saved-model"),
            ));
            cx.notify();
        })
    });
    cx.run_until_parked();
    // Electron's header always offers re-run; Retry belongs to a failed result.
    assert!(shown(cx, "copilot-rerun-saved-prompt"));
    assert!(!shown(cx, "copilot-retry-failed-result"));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.selected = Some(saved_result(
                "failed",
                serde_json::json!({"ghAccount": "saved-account"}),
                Some("saved-model"),
            ));
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(shown(cx, "copilot-retry-failed-result"));

    let temp = tempfile::tempdir().unwrap();
    let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    for _ in 0..2 {
        let calls = calls.clone();
        let temp = &temp;
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.rerun_with(cx, |options, record| {
                    calls.borrow_mut().push((
                        options.prompt.clone(),
                        options.saved,
                        options.context.account.clone(),
                        options.context.model.clone(),
                        options.review.is_none(),
                        record.category,
                        record.rerun_of.clone(),
                    ));
                    idle_run(temp)
                })
            })
        });
        cx.run_until_parked();
    }
    // The second attempt is refused while the first re-run is still busy.
    assert_eq!(
        *calls.borrow(),
        [(
            "Saved prompt 界".to_string(),
            true,
            "saved-account".to_string(),
            "saved-model".to_string(),
            true,
            None,
            Some("saved-result".to_string()),
        )]
    );
    cx.update(|cx| assert!(rerun::test_run(&view.read(cx).rerun).is_some()));
    assert!(shown(cx, "copilot-rerun-cancel"));
}

fn never_started() -> Result<
    (
        buddy_core::copilot_assistant::RunHandle,
        std::future::Ready<()>,
    ),
    buddy_core::copilot_runs::Failure,
> {
    panic!("A re-run without an account cannot start")
}

#[gpui_kit::test]
fn a_re_run_without_an_account_explains_why_and_starts_nothing(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.selected_id = Some("saved-result".into());
            view.selected = Some(saved_result("failed", serde_json::json!({}), None));
            view.rerun_with(cx, |_, _| never_started());
            assert!(view.rerun.error.as_ref().unwrap().contains("account"));
            assert!(rerun::test_run(&view.rerun).is_none());
        })
    });
}

#[gpui_kit::test]
fn a_pr_review_is_not_re_run_from_saved_results(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    let window = cx.update(|cx| cx.windows()[0]);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let mut review = saved_result(
                "failed",
                serde_json::json!({"ghAccount": "saved-account"}),
                Some("saved-model"),
            );
            review.category = Some("pr-review".into());
            view.selected_id = Some("saved-result".into());
            view.selected = Some(review);
            // Even a direct call starts nothing: the native session can't see the diff.
            view.rerun_with(cx, |_, _| never_started());
            assert!(rerun::test_run(&view.rerun).is_none());
            cx.notify();
        })
    });
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copilot-rerun-saved-prompt").is_none());
        assert!(window.try_find("copilot-retry-failed-result").is_none());
    })
    .unwrap();
}
