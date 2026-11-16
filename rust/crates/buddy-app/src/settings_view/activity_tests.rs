use super::*;
use gpui_kit::VisualTestContext;
use gpui_kit::test::TestWindowExt;
use std::{cell::Cell, rc::Rc};

#[gpui_kit::test]
fn category_clicks_count_repeated_opens_but_retained_restores_do_not(
    cx: &mut gpui_kit::TestAppContext,
) {
    let count = Rc::new(Cell::new(0));
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let count = count.clone();
            cx.new(|cx| {
                let mut view = SettingsView::new(window, cx);
                view.on_category_open = Rc::new(move |_| count.set(count.get() + 1));
                view
            })
        })
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    assert_eq!(count.get(), 0);
    visual.update(|window, cx| window.click("settings-panel-Appearance", cx));
    assert_eq!(
        count.get(),
        1,
        "the actual category button must record an explicit open"
    );
    let appearance = view.read_with(&visual, |state, cx| state.destination(view.clone(), cx));
    visual.update(|window, cx| window.click("settings-panel-Appearance", cx));
    visual.update(|window, cx| window.click("settings-panel-Accounts", cx));
    assert_eq!(count.get(), 3);
    visual.update(|window, cx| {
        let accounts = view.read(cx).destination(view.clone(), cx);
        let mut tabs = crate::shell::navigation::Tabs::default();
        tabs.open(appearance);
        tabs.open(accounts);
        tabs.travel(false);
        (tabs.current().unwrap().restore)(window, cx);
        assert!(view.read(cx).panel == Panel::Appearance);
        tabs.travel(true);
        (tabs.current().unwrap().restore)(window, cx);
        assert!(view.read(cx).panel == Panel::Accounts);
        view.update(cx, |state, cx| {
            state.activate(cx);
            cx.notify();
        });
    });
    visual.run_until_parked();
    assert_eq!(
        count.get(),
        3,
        "restore, activation and rendering are reads"
    );
    visual.update(|window, cx| {
        let input = view.read(cx).fields["enterprise"].clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        window.render_frame(cx);
        for _ in 0..32 {
            if window.find("settings-panel-Appearance").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(
            window.find("settings-panel-Appearance").focused(),
            Some(true)
        );
        window.press("enter", cx);
        assert_eq!(count.get(), 4, "category buttons remain keyboard reachable");
        window.press("space", cx);
        assert_eq!(
            count.get(),
            5,
            "repeated keyboard activation is another explicit open"
        );
    });
}
