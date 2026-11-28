//! Electron prompt-box parity: category, Recent list and active badge.
use super::*;
use buddy_core::copilot_backend::ActiveCounts;
use buddy_core::copilot_results::summaries::{ResultSummary, parse_results};
use buddy_core::copilot_runs::Failure;
use gpui_kit::test::TestWindowExt as _;
use std::cell::RefCell;
use std::rc::Rc;

fn open(cx: &mut gpui_kit::TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<ChatView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| ChatView::new(window, cx))
        })
        .unwrap()
    })
}

/// A real handle whose driver never runs. Nothing is started or written.
fn idle_run(temp: &tempfile::TempDir) -> Result<(RunHandle, std::future::Ready<()>), Failure> {
    copilot_assistant::start(RunOptions {
        context: RunContext {
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

pub(super) fn rows(prompts: &[(&str, &str, f64)]) -> Vec<ResultSummary> {
    let page: Vec<_> = prompts
        .iter()
        .enumerate()
        .map(|(index, (prompt, status, created))| {
            serde_json::json!({"_id": format!("result-{index}"), "promptPreview": prompt,
                "status": status, "createdAt": created, "hasOutput": false})
        })
        .collect();
    let wire = serde_json::json!({"page": page, "isDone": true, "continueCursor": ""});
    parse_results(&serde_json::to_vec(&wire).unwrap())
        .unwrap()
        .rows
}

#[gpui_kit::test]
fn a_saved_run_records_the_chosen_category_and_an_unsaved_turn_records_none(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    // Electron's options, in its order, under the prompt.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for index in 0..4usize {
            assert!(window.try_find(("copilot-category", index)).is_some());
        }
        window.click(("copilot-category", 3usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).category, Category::Documentation));
    let temp = tempfile::tempdir().unwrap();
    let recorded = Rc::new(RefCell::new(Vec::new()));
    for saved in [true, false] {
        let prompt = cx.update(|cx| view.read(cx).prompt.clone());
        let recorded = recorded.clone();
        let temp = &temp;
        cx.update_window(handle, |_, window, cx| {
            prompt.update(cx, |prompt, cx| {
                prompt.set_value("Synthetic prompt", window, cx)
            });
            view.update(cx, |view, cx| {
                view.run = None;
                view.send_with(
                    saved,
                    window,
                    cx,
                    |options, record| {
                        recorded.borrow_mut().push((
                            options.saved,
                            record.category,
                            record.rerun_of.clone(),
                        ));
                        idle_run(temp)
                    },
                    |_| {},
                )
            })
        })
        .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(
        *recorded.borrow(),
        [
            (true, Some(Category::Documentation), None),
            (false, None, None)
        ]
    );
}

#[gpui_kit::test]
fn a_captured_review_shows_pr_review_and_keeps_the_category_locked(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = open(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.category = Category::CodeAnalysis;
            view.review = Some(buddy_core::copilot_review::CapturedReview {
                snapshot: buddy_core::copilot_backend::ReviewSnapshot {
                    account: "synthetic-account".into(),
                    context: buddy_core::copilot_runs::ReviewContext {
                        repository: "owner/repo".into(),
                        pull_request: 7,
                        head: "a".repeat(40),
                    },
                    title: "Synthetic pull request".into(),
                    threads: buddy_core::copilot_results::summaries::ThreadStats {
                        total: 0,
                        unresolved: 0,
                        outdated: 0,
                    },
                },
                prompt: "Synthetic captured review".into(),
            });
            cx.notify();
        })
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("copilot-category", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(view.read(cx).category, Category::CodeAnalysis));
}

#[gpui_kit::test]
fn the_recent_list_shows_newest_results_with_status_and_time_and_opens_the_clicked_one(
    cx: &mut gpui_kit::TestAppContext,
) {
    let now = recent::now_ms();
    let long = "x".repeat(100);
    let sample = rows(&[
        ("Newest prompt", "running", now - 30_000.0),
        (&long, "failed", now - 3.0 * 3_600_000.0),
    ]);
    assert_eq!(recent::line(&sample[0], now), "🔄 Newest prompt · just now");
    assert_eq!(
        recent::line(&sample[1], now),
        format!("❌ {}… · 3 hours ago", "x".repeat(80))
    );

    let (handle, view) = open(cx);
    let opened = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let opened = opened.clone();
        cx.subscribe(&view, move |_, event: &ChatEvent, _| {
            if let ChatEvent::OpenResult(id) = event {
                opened.borrow_mut().push(id.clone());
            }
        })
        .detach();
        view.update(cx, |view, cx| {
            // Eleven rows arrive; Electron lists ten.
            let mut many = sample.clone();
            many.extend(rows(&[("Older", "completed", now - 60_000.0); 9]));
            view.recent
                .apply(Some(recent::fetched(many, Some(ActiveCounts::default()))));
            cx.notify();
        })
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("copilot-recent-row", 9usize)).is_some());
        assert!(window.try_find(("copilot-recent-row", 10usize)).is_none());
        window.click(("copilot-recent-row", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*opened.borrow(), ["result-1"]);
    // During a conversation the conversation keeps that space.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.messages.push(super::Message {
                user: true,
                partial: false,
                unconfirmed: false,
                text: "Synthetic question".into(),
            });
            cx.notify();
        })
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("copilot-recent-row", 0usize)).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_prompt_header_shows_the_active_count_only_while_prompts_are_active(
    cx: &mut gpui_kit::TestAppContext,
) {
    let mut recent = Recent::default();
    recent.apply(Some(recent::fetched(
        Vec::new(),
        Some(ActiveCounts {
            pending: 1,
            running: 2,
        }),
    )));
    assert_eq!(recent.badge().as_deref(), Some("3 active"));
    assert!(!recent.unavailable);
    // A failed count read leaves the fresh rows fresh; only the count is
    // unknown, so it is not shown.
    recent.apply(Some(recent::fetched(Vec::new(), None)));
    assert_eq!(recent.badge(), None);
    assert!(!recent.unavailable);
    // A failed rows read says the list is stale, and a count read with it
    // still shows.
    recent.apply(Some(recent::failed_rows(Some(ActiveCounts {
        pending: 2,
        running: 0,
    }))));
    assert_eq!(recent.badge().as_deref(), Some("2 active"));
    assert!(recent.unavailable);
    recent.apply(Some(recent::fetched(
        Vec::new(),
        Some(ActiveCounts::default()),
    )));
    assert_eq!(recent.badge(), None);
    // A split page is shown with a note that newer results may be missing.
    recent.apply(Some(recent::fetched_page(Vec::new(), true, None)));
    assert!(recent.partial);
    recent.apply(Some(recent::fetched(Vec::new(), None)));
    assert!(!recent.partial);

    // The header shows it only in the Copilot pane, never in the sidebar assistant.
    let (_, view) = open(cx);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.recent.apply(Some(recent::fetched(
                Vec::new(),
                Some(ActiveCounts {
                    pending: 0,
                    running: 1,
                }),
            )));
            assert_eq!(view.header_badge().as_deref(), Some("1 active"));
            view.sidebar_context = Some(crate::assistant_context::AssistantContext::section(None));
            assert_eq!(view.header_badge(), None);
        })
    });
}

#[gpui_kit::test]
fn a_refresh_asked_for_while_loading_waits_and_the_sidebar_never_reads(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (_, view) = open(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.recent.loading = true;
            view.refresh_recent(cx);
            assert!(view.recent.stale);
            assert!(view.recent.loading);
            view.recent.loading = false;
            view.recent.stale = false;
            view.sidebar_context = Some(crate::assistant_context::AssistantContext::section(None));
            view.refresh_recent(cx);
            assert!(!view.recent.loading);
        })
    });
}

#[gpui_kit::test]
fn recent_keeps_refresh_after_an_empty_successful_read(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view) = open(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.recent.apply(Some(recent::fetched(
                Vec::new(),
                Some(ActiveCounts::default()),
            )));
            cx.notify();
        })
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copilot-recent-refresh").is_some());
    })
    .unwrap();
}
