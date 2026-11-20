use super::*;
use buddy_core::github::{AccountScope, ApiRequest, ApiResponse, GitHubError, Transport};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::{future::Future, pin::Pin};

#[gpui_kit::test]
fn publication_panes_stay_below_filters_at_compact_and_large_sizes(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions, px, size};

    for (width, height) in [(900., 650.), (1200., 820.)] {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut config = buddy_core::config::AppConfig::default();
            config.github.accounts = vec![serde_json::from_value(json!({
                "username":"fixture-user", "org":"fixture-org"
            })).unwrap()];
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            // No Runtime: layout and preparing confirmation cannot access a service.
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(), size: size(px(width), px(height)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| {
                    let mut view = ResultsView::new(window, cx);
                    view.initialized = true;
                    view.selected_id = Some("saved-review".into());
                    view.selected = Some(serde_json::from_value(json!({
                        "_id":"saved-review",
                        "prompt":"Review this pull request for correctness, regressions, security, and missing validation. Return actionable findings as Markdown, with file and line references and a concise summary. You have no tools",
                        "result":"## Synthetic assistant\n\nSaved review paragraph.\n\n".repeat(30),
                        "category":"pr-review", "status":"completed", "model":"fixture-model", "createdAt":1,
                        "metadata":{"owner":"fixture-org", "repo":"native-demo", "prNumber":7,
                            "ghAccount":"fixture-user", "reviewedHeadSha":"a".repeat(40)}
                    })).unwrap());
                    view.rows = vec![serde_json::from_value(json!({
                        "_id":"saved-review", "promptPreview":"Synthetic saved review", "status":"completed",
                        "modelLabel":"fixture-model", "categoryLabel":"pr-review", "createdAt":1, "hasOutput":true
                    })).unwrap()];
                    view.prepare_publication(cx);
                    view
                }),
            ).unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            for status in [None, Some("Review comment published. This session will not publish this result again."),
                Some("Publication unconfirmed. Check the PR on GitHub before any further action. This session will not resend it.")] {
                if let Some(status) = status {
                    view.update(cx, |view, cx| {
                        view.publication.pending = None;
                        view.publication.attempts.insert("saved-review".into(), status);
                        cx.notify();
                    });
                }
                window.render_frame(cx);
                let paging = window.find("copilot-results-newest").bounds();
                let detail = window.find("copilot-refresh-result").bounds();
                let list = window.find(("copilot-saved-row", 0usize)).bounds();
                assert!(detail.origin.y >= paging.bottom(),
                    "Saved detail overlaps pagination at {width}x{height}: {detail:?}, paging {paging:?}");
                assert_eq!(detail.origin.y, list.origin.y);
                assert!(detail.bottom() <= window.viewport_size().height);
            }
            assert!(!view.read(cx).write_loading);
            assert!(!cx.has_global::<Runtime>());
        }).unwrap();
    }
}

struct Fixture {
    calls: Mutex<Vec<(String, ApiRequest)>>,
    uncertain: bool,
}
impl Transport for Fixture {
    fn request<'a>(
        &'a self,
        account: &'a AccountScope,
        request: ApiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResponse, GitHubError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((account.key(), request));
            if self.uncertain {
                Err(GitHubError::UnknownWriteResult)
            } else {
                Ok(ApiResponse {
                    status: 201,
                    headers: Default::default(),
                    body: json!({"id":301}),
                })
            }
        })
    }
}
fn setup(
    cx: &mut gpui_kit::TestAppContext,
    uncertain: bool,
) -> (Entity<ResultsView>, Arc<Fixture>) {
    let fixture = Arc::new(Fixture {
        calls: Mutex::default(),
        uncertain,
    });
    let clients = GitHubClients::with_transport(fixture.clone());
    let view = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.github.accounts = vec![serde_json::from_value(json!({"username":"fixture-user","org":"fixture-org"})).unwrap()];
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window,cx|
            cx.new(|cx| ResultsView::new(window,cx))).unwrap();
        view.update(cx, |view,_| {
            view.publication.clients = clients;
            view.selected_id = Some("saved-review".into());
            view.selected = Some(serde_json::from_value(json!({"_id":"saved-review","prompt":"private prompt", "result":"Saved answer", "category":"pr-review", "status":"completed", "model":"fixture-model", "createdAt":1, "metadata":{"owner":"fixture-org","repo":"native-demo","prNumber":7,"ghAccount":"fixture-user","reviewedHeadSha":"a".repeat(40)}})).unwrap());
        });
        view
    });
    (view, fixture)
}
#[gpui_kit::test]
fn one_confirmed_comment_retains_exact_scope_body_and_never_replays(
    cx: &mut gpui_kit::TestAppContext,
) {
    for uncertain in [false, true] {
        let (view, fixture) = setup(cx, uncertain);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.prepare_publication(cx);
                assert!(
                    fixture.calls.lock().unwrap().is_empty(),
                    "Preparing must not send"
                );
                view.publication.pending = None; // The production Cancel action.
                view.confirm_publication(cx);
                assert!(
                    fixture.calls.lock().unwrap().is_empty(),
                    "Cancelled confirmation must not send"
                );
                view.prepare_publication(cx);
                view.confirm_publication(cx);
                view.confirm_publication(cx); // Duplicate click before worker completion.
            })
        });
        cx.run_until_parked();
        cx.update(|cx| view.update(cx, |view,cx| {
            assert!(!view.busy());
            let status = view.publication.attempts.get("saved-review").unwrap();
            assert!(status.contains(if uncertain { "unconfirmed" } else { "published" }));
            view.selected = None; // Navigation away, followed by restoring the saved result.
            view.selected = Some(serde_json::from_value(json!({"_id":"saved-review","prompt":"private prompt","result":"Saved answer","category":"pr-review","status":"completed","createdAt":1,"metadata":{"org":"fixture-org","repo":"native-demo","prNumber":7}})).unwrap());
            view.prepare_publication(cx);
            assert!(view.publication.pending.is_none());
            view.confirm_publication(cx);
        }));
        cx.run_until_parked();
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "Neither uncertain nor confirmed attempts may be replayed"
        );
        assert_eq!(calls[0].0, "fixture-org/fixture-user");
        assert_eq!(calls[0].1.method, reqwest::Method::POST);
        assert_eq!(
            calls[0].1.path,
            "/repos/fixture-org/native-demo/issues/7/comments"
        );
        assert_eq!(
            calls[0].1.body.as_ref().unwrap()["body"],
            "## 🤖 AI Review\n\nSaved answer\n\n---\n*Published from HS Buddy — fixture-model review*"
        );
    }
}
#[gpui_kit::test]
fn changed_answer_selection_and_account_configuration_disarm_confirmation(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (view, fixture) = setup(cx, false);
    for change in 0..3 {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.prepare_publication(cx);
                assert!(view.publication.pending.is_some());
                match change {
                    0 => {
                        view.selected.as_mut().unwrap().result = Some("Changed saved answer".into())
                    }
                    1 => view.selected.as_mut().unwrap().id = "another-record".into(),
                    _ => cx.global_mut::<Settings>().config.github.accounts.clear(),
                }
                view.confirm_publication(cx);
                assert!(view.publication.pending.is_none());
            })
        });
        cx.run_until_parked();
        assert!(fixture.calls.lock().unwrap().is_empty());
    }
}
#[gpui_kit::test]
fn settings_change_back_still_discards_the_old_confirmation(cx: &mut gpui_kit::TestAppContext) {
    let (view, fixture) = setup(cx, false);
    cx.update(|cx| view.update(cx, |view, cx| view.prepare_publication(cx)));
    cx.update(|cx| {
        cx.global_mut::<Settings>().config.github.accounts[0].username = "changed".into();
    });
    cx.run_until_parked();
    cx.update(|cx| {
        cx.global_mut::<Settings>().config.github.accounts[0].username = "fixture-user".into();
    });
    cx.run_until_parked();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            assert!(view.publication.pending.is_none());
            view.confirm_publication(cx);
        })
    });
    cx.run_until_parked();
    assert!(fixture.calls.lock().unwrap().is_empty());
}
