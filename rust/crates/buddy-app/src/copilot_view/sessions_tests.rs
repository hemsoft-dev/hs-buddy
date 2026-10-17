use super::*;
use buddy_core::copilot_sessions::RequestResult;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Point, VisualTestContext, WindowBounds, WindowOptions, px, size};

#[gpui_kit::test]
fn compact_session_status_reserves_space_below_reachable_request_paging(
    cx: &mut gpui_kit::TestAppContext,
) {
    // The app frame, rail, navigation and padding leave this body at 900x650.
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(812.0), px(484.0)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = SessionsView::new(window, cx);
                    let session = SessionDetail {
                        session_id: "layout-fixture".into(),
                        title: "Retained session".into(),
                        start_time: 0,
                        model: None,
                        results: (0..21)
                            .map(|i| RequestResult {
                                prompt: format!("Synthetic request {}", i + 1),
                                ..Default::default()
                            })
                            .collect(),
                        malformed_lines: 0,
                    };
                    view.digest = Some(session.digest("project", "", 0));
                    view.detail = Some(session);
                    view.loaded_source = Some(PathBuf::from(format!(
                        "/retained/{}/editor User directory 界",
                        "long Unicode workspace 界/".repeat(5)
                    )));
                    view.error =
                        Some("The selected VS Code session directory is unavailable.".into());
                    view
                })
            },
        )
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let pager = visual.debug_bounds("session-request-pager").unwrap();
    let source = visual.debug_bounds("session-loaded-source").unwrap();
    let error = visual.debug_bounds("session-source-error").unwrap();
    assert!(
        pager.bottom() + px(8.0) <= source.top(),
        "Request paging must remain above the retained source, pager={pager:?}, source={source:?}"
    );
    assert!(
        source.bottom() < error.top(),
        "Source and error must not overlap"
    );
    assert!(
        error.bottom() <= px(484.0),
        "Status must remain inside the compact body"
    );
    assert!(
        source.size.height > px(30.0),
        "The complete Unicode source wraps"
    );

    let content_before = visual.debug_bounds("session-request-0").unwrap();
    let viewport = visual.debug_bounds("session-detail-viewport").unwrap();
    visual.simulate_event(gpui_kit::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-900.0))),
        ..Default::default()
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("session-request-0").unwrap().top() < content_before.top());
    assert_eq!(visual.debug_bounds("session-request-pager").unwrap(), pager);
    assert_eq!(
        visual.debug_bounds("session-loaded-source").unwrap(),
        source
    );

    visual.update(|window, cx| window.click("copilot-session-next-requests", cx));
    visual.update(|_, cx| assert_eq!(view.read(cx).request_page, 1));
    // Actual Tab traversal must reach the now-enabled Previous control.
    visual.update(|window, cx| {
        let path = view.read(cx).path.clone();
        path.update(cx, |state, cx| state.focus(window, cx));
        window.draw(cx).clear(cx);
        for _ in 0..12 {
            if window.find("copilot-session-previous-requests").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(
            window.find("copilot-session-previous-requests").focused(),
            Some(true)
        );
        window.press("enter", cx);
        let state = view.read(cx);
        assert_eq!(state.request_page, 0);
        assert_eq!(state.detail.as_ref().unwrap().results.len(), 21);
        assert!(
            state
                .loaded_source
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .ends_with("editor User directory 界")
        );
        assert_eq!(
            state.error.as_deref(),
            Some("The selected VS Code session directory is unavailable.")
        );
    });
}
