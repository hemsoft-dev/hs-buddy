//! macOS application roles complement the frameless window's custom menus.
use gpui_kit::base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{App, Div, InteractiveElement, KeyBinding, Menu, MenuItem, OsAction};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSApplication;

use crate::app::{About, Quit, ToggleFullScreen};
use crate::settings::Settings;

mod edit;

gpui_kit::actions!(
    macos,
    [
        OpenSettings,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        CloseWindow,
        BringToFront
    ]
);

pub fn bind_keys(cx: &mut App) {
    // Install before F11 so the native menu displays the macOS equivalent.
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);
}

pub fn show_window(cx: &mut App) {
    if cx.windows().is_empty() {
        crate::open_when_displays_ready(cx);
    } else {
        for handle in cx.windows() {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
        cx.activate(true);
    }
}

pub fn window_actions(root: Div) -> Div {
    // Scope these actions to the live window. AppKit's menu validation then
    // disables them automatically after the last window is closed.
    root.on_action(|_: &Minimize, window, _| window.minimize_window())
        .on_action(|_: &Zoom, window, _| window.zoom_window())
        .on_action(|_: &CloseWindow, window, _| window.remove_window())
        .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
        .on_action(|_: &BringToFront, window, cx| {
            window.activate_window();
            cx.activate(true);
        })
}

fn menus(window_open: bool) -> Vec<Menu> {
    vec![
        Menu::new("Buddy Native").items([
            MenuItem::action("About Buddy", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            // Register the actual NSMenu below. GPUI 0.3.9's SystemMenu path
            // passes an NSMenuItem to setServicesMenu:, which expects NSMenu.
            MenuItem::submenu(Menu::new("Services")),
            MenuItem::separator(),
            MenuItem::action("Hide Buddy Native", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Buddy Native", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize).disabled(!window_open),
            MenuItem::action("Zoom", Zoom).disabled(!window_open),
            MenuItem::action("Close Window", CloseWindow).disabled(!window_open),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringToFront).disabled(!window_open),
        ]),
    ]
}

pub fn install(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| {
        select_settings(cx, |cx| {
            Settings::update(cx, |config| {
                config.native.active_section = Some("settings".into())
            })
        });
        // A failed save keeps save_error for the visible status bar. Always
        // reopen so a windowless app can display that error and recover.
        show_window(cx);
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    refresh(cx);
}

fn select_settings(cx: &mut App, persist: impl FnOnce(&mut App) -> bool) {
    if !persist(cx) {
        Settings::stage_native_section(cx, "settings");
    }
    // Navigation is volatile UI state even when persistence cannot read or
    // lock the profile. Preserve save_error and all other loaded settings.
    cx.refresh_windows();
}

pub fn refresh(cx: &mut App) {
    cx.set_menus(menus(!cx.windows().is_empty()));
    install_responder_roles(cx);
}

// NSMenu holds its delegate weakly. Keep the adapter alive with the app.
struct MenuDelegateOwner {
    _delegate: Retained<edit::EditMenuDelegate>,
}
impl gpui_kit::Global for MenuDelegateOwner {}

fn install_responder_roles(cx: &mut App) {
    // GPUI's native view is a child of NSWindow.contentView, not the content
    // view itself. Use its public raw handle to distinguish native responders.
    let gpui_view = cx.windows().first().copied().and_then(|handle| {
        handle
            .update(cx, |_, window, _| {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                match <gpui_kit::Window as HasWindowHandle>::window_handle(window)
                    .ok()?
                    .as_raw()
                {
                    RawWindowHandle::AppKit(handle) => Some(handle.ns_view.as_ptr() as usize),
                    _ => None,
                }
            })
            .ok()
            .flatten()
    });
    let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("GPUI main thread"));
    let Some(bar) = app.mainMenu() else { return };
    if let Some(menu) = bar.itemAtIndex(0).and_then(|item| item.submenu()) {
        for item in menu.itemArray() {
            if item.title().to_string() == "Services" {
                app.setServicesMenu(item.submenu().as_deref());
            }
        }
    }
    if let Some(menu) = bar.itemAtIndex(1).and_then(|item| item.submenu()) {
        let delegate = edit::EditMenuDelegate::new(&menu, gpui_view);
        cx.set_global(MenuDelegateOwner {
            _delegate: delegate,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn unreadable_profile_still_selects_settings_without_losing_the_save_error(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, "{invalid synthetic fixture").unwrap();
        let edit_called = std::cell::Cell::new(false);
        cx.update(|cx| {
            cx.set_global(Settings::new(buddy_core::config::AppConfig::default()));
            select_settings(cx, |cx| {
                let saved = Settings::update_at(cx, &path, |config| {
                    edit_called.set(true);
                    config.native.active_section = Some("settings".into());
                });
                assert!(!saved);
                saved
            });
            assert!(!edit_called.get(), "Unreadable files fail before editing");
            assert_eq!(
                Settings::global(cx).config.native.active_section.as_deref(),
                Some("settings")
            );
            assert!(
                Settings::global(cx)
                    .save_error
                    .as_deref()
                    .unwrap()
                    .contains("could not be read")
            );
        });
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{invalid synthetic fixture"
        );
        let mut restored = buddy_core::config::AppConfig::default();
        restored.extra.insert("future".into(), serde_json::json!(7));
        std::fs::write(&path, serde_json::to_vec(&restored).unwrap()).unwrap();
        cx.update(|cx| {
            assert!(Settings::update_at(cx, &path, |config| {
                config.pr.recently_merged_days = 17.0;
            }));
            assert_eq!(
                Settings::global(cx).config.native.active_section.as_deref(),
                Some("settings")
            );
            assert!(Settings::global(cx).save_error.is_none());
        });
        let saved = buddy_core::config::AppConfig::load_from(&path).unwrap();
        assert_eq!(saved.native.active_section.as_deref(), Some("settings"));
        assert_eq!(saved.pr.recently_merged_days, 17.0);
        assert_eq!(saved.extra["future"], 7);
    }

    #[test]
    fn native_menus_keep_edit_responder_roles_and_window_registration() {
        let menus = menus(true);
        assert_eq!(
            menus.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
            ["Buddy Native", "Edit", "Window"]
        );
        let roles = menus[1]
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { os_action, .. } => *os_action,
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            roles
                == [
                    OsAction::Undo,
                    OsAction::Redo,
                    OsAction::Cut,
                    OsAction::Copy,
                    OsAction::Paste,
                    OsAction::SelectAll
                ]
        );
        assert!(matches!(&menus[0].items[4], MenuItem::Submenu(menu) if menu.name == "Services"));
    }

    #[test]
    fn window_commands_disable_when_the_last_window_closes() {
        for open in [true, false] {
            let menus = menus(open);
            for item in &menus[2].items {
                if let MenuItem::Action { disabled, .. } = item {
                    assert_eq!(*disabled, !open);
                }
            }
            assert!(menus[0].items.iter().all(|item| !item.is_disabled()));
        }
    }

    #[gpui_kit::test]
    fn macos_accelerators_use_command_and_preserve_the_fullscreen_alternative(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            bind_keys(cx);
            cx.bind_keys([KeyBinding::new("f11", ToggleFullScreen, None)]);
            let bindings = cx.key_bindings();
            let bindings = bindings.borrow();
            for (key, action) in [
                ("cmd-,", Box::new(OpenSettings) as Box<dyn gpui_kit::Action>),
                ("cmd-h", Box::new(Hide)),
                ("cmd-alt-h", Box::new(HideOthers)),
                ("cmd-m", Box::new(Minimize)),
                ("cmd-w", Box::new(CloseWindow)),
                ("ctrl-cmd-f", Box::new(ToggleFullScreen)),
            ] {
                assert!(
                    bindings
                        .bindings_for_action(action.as_ref())
                        .any(|binding| binding.keystrokes().len() == 1
                            && binding.keystrokes()[0].inner()
                                == &gpui_kit::Keystroke::parse(key).unwrap())
                );
            }
            assert_eq!(bindings.bindings_for_action(&ToggleFullScreen).count(), 2);
        });
    }
}
