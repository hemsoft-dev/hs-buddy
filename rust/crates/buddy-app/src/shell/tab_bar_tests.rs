fn global_tab_context_fixture(
    cx: &mut gpui_kit::TestAppContext,
) -> (
    gpui_kit::AnyWindowHandle,
    Entity<BuddyApp>,
    std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
) {
    let (window, shell) = status_click_shell(cx);
    let closed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    cx.update(|cx| {
        shell.update(cx, |app, cx| {
            app.tabs.close_all();
            for key in ["one", "two", "three"] {
                let mut tab =
                    crate::shell::navigation::Destination::section(Some(Section::Insights));
                tab.key = key.into();
                tab.label = format!("Workspace {key}");
                let closed = closed.clone();
                tab.close = std::rc::Rc::new(move |_| closed.borrow_mut().push(key));
                app.tabs.open(tab);
            }
            app.tabs.select("one");
            app.active_section = Some(Section::Insights);
            cx.notify();
        });
    });
    (window, shell, closed)
}

#[gpui_kit::test]
fn global_tab_context_right_click_targets_inactive_tab(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child("app::tests::global_tab_context_right_click_targets_inactive_tab") {
        return;
    }
    let (handle, shell, closed) = global_tab_context_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        crate::browser_view::Overlays::set(true, cx);
        window.render_frame(cx);
        window.right_click(("global-tab", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("popup-menu").visible());
        assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("one"));
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 2);
        window.within("popup-menu").click(2usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let tabs = &shell.read(cx).tabs;
        assert_eq!(
            tabs.items
                .iter()
                .map(|tab| tab.key.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(tabs.active.as_deref(), Some("one"));
        assert_eq!(&*closed.borrow(), &["three"]);
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 1);
        crate::browser_view::Overlays::set(false, cx);
    });
}

#[gpui_kit::test]
fn global_tab_context_escape_and_dropdown_use_the_same_target(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child("app::tests::global_tab_context_escape_and_dropdown_use_the_same_target")
    {
        return;
    }
    let (handle, shell, closed) = global_tab_context_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("global-tab", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
        assert_eq!(shell.read(cx).tabs.items.len(), 3);
        assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("one"));
        assert!(closed.borrow().is_empty());
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 0);
        window.click(("global-tab-menu", 1usize), cx);
        window.within("popup-menu").click(1usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(shell.read(cx).tabs.items.len(), 1);
        assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("two"));
        assert_eq!(&*closed.borrow(), &["one", "three"]);
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 0);
    });
}

#[gpui_kit::test]
fn global_tab_context_disabled_bulk_commands_reject_pointer_and_skip_keyboard(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child(
        "app::tests::global_tab_context_disabled_bulk_commands_reject_pointer_and_skip_keyboard",
    ) {
        return;
    }
    let (handle, shell, closed) = global_tab_context_fixture(cx);
    for count in [3, 1] {
        cx.update_window(handle, |_, window, cx| {
            shell.update(cx, |app, cx| {
                app.tabs.items.truncate(count);
                cx.notify();
            });
            window.render_frame(cx);
            window.right_click(("global-tab", count - 1), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("popup-menu").click(2usize, cx);
            assert!(
                window.find("popup-menu").visible(),
                "disabled command dismissed the menu"
            );
            assert_eq!(shell.read(cx).tabs.items.len(), count);
            assert!(closed.borrow().is_empty());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(("global-tab", count - 1), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("down", cx);
            window.press("down", cx);
            if count == 3 {
                assert_eq!(
                    window.within("popup-menu").find(1usize).selected(),
                    Some(true)
                );
                window.press("down", cx);
            }
            assert_eq!(
                window.within("popup-menu").find(3usize).selected(),
                Some(true)
            );
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 0));
    }
}

#[gpui_kit::test]
fn global_tab_context_stale_close_all_keeps_replacement_tabs(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child("app::tests::global_tab_context_stale_close_all_keeps_replacement_tabs") {
        return;
    }
    let (handle, shell, closed) = global_tab_context_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("global-tab", 2usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        shell.update(cx, |app, cx| {
            app.tabs.close("three");
            let mut replacement =
                crate::shell::navigation::Destination::section(Some(Section::Insights));
            replacement.key = "four".into();
            replacement.label = "Replacement workspace".into();
            app.tabs.open(replacement);
            app.tabs.select("one");
            app.restore_tab(window, cx);
        });
        window.render_frame(cx);
        window.within("popup-menu").click(3usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let app = shell.read(cx);
        assert_eq!(
            app.tabs
                .items
                .iter()
                .map(|tab| tab.key.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "four"]
        );
        assert_eq!(&*closed.borrow(), &["three"]);
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 0);
    });
}

#[gpui_kit::test]
fn global_tab_context_removing_trigger_releases_browser_guard(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child("app::tests::global_tab_context_removing_trigger_releases_browser_guard")
    {
        return;
    }
    let (handle, shell, _) = global_tab_context_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("global-tab", 2usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 1);
        shell.update(cx, |app, cx| {
            app.tabs.close("three");
            app.restore_tab(window, cx);
        });
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 0));
}

#[gpui_kit::test]
fn global_tab_context_reopen_and_outside_dismiss_preserve_foreign_overlay(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    if status_click_child(
        "app::tests::global_tab_context_reopen_and_outside_dismiss_preserve_foreign_overlay",
    ) {
        return;
    }
    let (handle, shell, closed) = global_tab_context_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        crate::browser_view::Overlays::set(true, cx);
        window.render_frame(cx);
    })
    .unwrap();
    for cycle in 0..3 {
        cx.update_window(handle, |_, window, cx| {
            window.click(("global-tab-menu", 1usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("popup-menu").visible());
            assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 2);
            if cycle == 1 {
                window.click(("global-tab-menu", 1usize), cx);
            } else {
                window.press("escape", cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 1));
    }
    cx.update_window(handle, |_, window, cx| {
        window.right_click(("global-tab", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 2);
        window.click(("global-tab", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
        assert_eq!(shell.read(cx).tabs.items.len(), 3);
        assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("one"));
        assert!(closed.borrow().is_empty());
        assert_eq!(cx.global::<crate::browser_view::Overlays>().0, 1);
        crate::browser_view::Overlays::set(false, cx);
    })
    .unwrap();
}
