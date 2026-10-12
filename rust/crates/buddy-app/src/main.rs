// A desktop app: release builds on Windows must not open a console window
// next to the GPUI window. Debug builds keep it so `RUST_LOG` output shows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod dashboard;
mod logging;
mod platform_placement;
mod runtime;
mod settings;
mod shell;
mod theme;
mod window_geometry;
mod window_persistence;
mod zoom;

use buddy_core::config::{AppConfig, WindowState};
use gpui_kit::component::TitleBar;
use gpui_kit::{
    App, AppContext as _, Bounds, DisplayId, KeyBinding, Pixels, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions, point, px, size,
};

use crate::app::{About, BuddyApp, Quit, Reload, ToggleFullScreen};
use crate::assets::BuddyAssets;
use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::window_geometry::{DisplayArea, resolve_window_bounds};
use gpui_kit::component::WindowExt as _;

/// Reuse the geometry saved in `window-state.json` (by this app or Electron)
/// when it still lands on a connected display; otherwise center a
/// default-sized window. The flag asks for full screen once the window is
/// shown: a window that was maximized underneath full screen opens
/// maximized first, so leaving full screen returns to maximized.
struct InitialPlacement {
    bounds: WindowBounds,
    enter_full_screen: bool,
    display_id: Option<DisplayId>,
    restore_outer: Option<Bounds<Pixels>>,
}

/// Match stable IDs instead of rectangles: macOS screens can have identical
/// local bounds. A disconnected display deliberately falls back to primary.
fn saved_display_index(saved: Option<&str>, connected: &[Option<String>]) -> Option<usize> {
    let saved = saved?;
    connected
        .iter()
        .position(|uuid| uuid.as_deref() == Some(saved))
}

fn initial_window_bounds(cx: &App) -> InitialPlacement {
    let area = |display: &std::rc::Rc<dyn gpui_kit::PlatformDisplay>| DisplayArea {
        bounds: display.bounds(),
        work_area: display.visible_bounds(),
    };
    let connected = cx.displays();
    let displays: Vec<DisplayArea> = connected.iter().map(area).collect();
    let primary = cx.primary_display().map(|display| area(&display));
    let default_size = size(px(1280.0), px(820.0));
    // First launch (or an unusable saved state): the default size fitted to
    // and centred on the primary work area, so a 1366×768 screen gets a
    // window that fits rather than one hanging past the taskbar.
    let fallback = || {
        let default = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: default_size,
        };
        let placed = resolve_window_bounds(default, &[], primary.as_ref())
            .unwrap_or_else(|| Bounds::centered(None, default_size, cx));
        InitialPlacement {
            bounds: WindowBounds::Windowed(placed),
            enter_full_screen: false,
            display_id: cx.primary_display().map(|display| display.id()),
            restore_outer: None,
        }
    };
    let Some(state) = WindowState::load() else {
        return fallback();
    };
    let saved: Bounds<Pixels> = Bounds {
        origin: point(px(state.x as f32), px(state.y as f32)),
        size: size(px(state.width as f32), px(state.height as f32)),
    };
    // Displays may have moved or shrunk since the state was written: fit the
    // window to the matching display's work area (`resolveWindowBounds`).
    let uuids: Vec<_> = connected
        .iter()
        .map(|display| display.uuid().ok().map(|id| id.to_string()))
        .collect();
    let saved_display = saved_display_index(state.native_display_uuid.as_deref(), &uuids)
        .map(|index| &connected[index]);
    // macOS rectangles are screen-local, and all screens have origin zero.
    // A disconnected saved UUID must resolve against the primary display,
    // rather than arbitrarily matching another screen with the same size.
    let selected = if cfg!(target_os = "macos") {
        saved_display.cloned().or_else(|| cx.primary_display())
    } else {
        saved_display.cloned()
    };
    let selected_area = selected.as_ref().map(area);
    let candidates: &[DisplayArea] = if cfg!(target_os = "macos") {
        selected_area.as_slice()
    } else {
        &displays
    };
    let Some(bounds) = resolve_window_bounds(saved, candidates, primary.as_ref()) else {
        return fallback();
    };
    let (window_bounds, enter_full_screen) = match (state.is_maximized, state.is_full_screen) {
        (true, full_screen) => (WindowBounds::Maximized(bounds), full_screen),
        (false, true) if cfg!(windows) => (WindowBounds::Windowed(bounds), true),
        (false, true) => (WindowBounds::Fullscreen(bounds), false),
        (false, false) => (WindowBounds::Windowed(bounds), false),
    };
    InitialPlacement {
        bounds: window_bounds,
        enter_full_screen,
        display_id: selected.map(|display| display.id()),
        restore_outer: cfg!(windows).then_some(bounds),
    }
}

fn main() {
    logging::init();

    let config = match AppConfig::load() {
        Ok(config) => config,
        Err(err) => {
            log::warn!("using default configuration: {err}");
            AppConfig::default()
        }
    };

    gpui_kit::application()
        .with_assets(BuddyAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::install(&config, cx);
            cx.set_global(Settings {
                config: config.clone(),
            });
            cx.set_global(Runtime::start());
            zoom::init(cx);

            cx.bind_keys([
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("f11", ToggleFullScreen, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &ToggleFullScreen, cx| {
                if let Some(window) = cx.active_window() {
                    window
                        .update(cx, |_, window, _| window.toggle_fullscreen())
                        .ok();
                }
            });
            cx.on_action(|_: &Reload, cx| match AppConfig::load() {
                Ok(config) => {
                    theme::install(&config, cx);
                    Settings::replace(cx, config);
                    cx.refresh_windows();
                }
                Err(err) => log::warn!("reload failed: {err}"),
            });
            cx.on_action(|_: &About, cx| {
                if let Some(window) = cx.active_window() {
                    window
                        .update(cx, |_, window, cx| {
                            window.open_alert_dialog(cx, |alert, _, _| {
                                alert.title("About Buddy").description(format!(
                                    "Buddy {} (native GPUI build)\nYour Universal Productivity Companion\nMade with love by HemSoft Developments",
                                    env!("BUDDY_VERSION")
                                ))
                            })
                        })
                        .ok();
                }
            });
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let placement = initial_window_bounds(cx);
            let options = WindowOptions {
                window_bounds: Some(placement.bounds),
                display_id: placement.display_id,
                window_min_size: Some(size(px(800.0), px(600.0))),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("com.hemsoft.buddy".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Buddy".into()),
                    ..TitleBar::title_bar_options()
                }),
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, |window, cx| {
                if let Some(bounds) = placement.restore_outer {
                    platform_placement::apply_restore_bounds(window, bounds);
                }
                // Closing the only window quits the app, so quit while the
                // window still exists instead of letting the OS destroy it
                // first: GPUI's Windows teardown (`WindowsWindow::drop`) then
                // runs on a live HWND rather than logging invalid-handle
                // errors for an already destroyed one, and the title-bar
                // close button takes the same path as File → Exit.
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    false
                });
                if placement.enter_full_screen {
                    window.on_next_frame(|window, _| window.toggle_fullscreen());
                }
                cx.new(|cx| BuddyApp::new(window, cx))
            })
            .expect("failed to open the Buddy window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    #[test]
    fn secondary_display_is_matched_by_uuid_not_enumeration_order() {
        let displays = vec![Some("primary".into()), Some("secondary".into())];
        assert_eq!(saved_display_index(Some("secondary"), &displays), Some(1));
        let reordered = vec![Some("secondary".into()), Some("primary".into())];
        assert_eq!(saved_display_index(Some("secondary"), &reordered), Some(0));
    }

    #[test]
    fn disconnected_or_legacy_display_uses_the_geometry_fallback() {
        let displays = vec![Some("primary".into()), None];
        assert_eq!(saved_display_index(Some("disconnected"), &displays), None);
        assert_eq!(saved_display_index(None, &displays), None);
        assert_eq!(saved_display_index(Some("secondary"), &[]), None);
    }
}
