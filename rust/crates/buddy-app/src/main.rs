// A desktop app: release builds on Windows must not open a console window
// next to the GPUI window. Debug builds keep it so `RUST_LOG` output shows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod dashboard;
mod logging;
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
    App, AppContext as _, Bounds, KeyBinding, Pixels, TitlebarOptions, WindowBounds,
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
fn initial_window_bounds(cx: &App) -> (WindowBounds, bool) {
    let area = |display: &std::rc::Rc<dyn gpui_kit::PlatformDisplay>| DisplayArea {
        bounds: display.bounds(),
        work_area: display.visible_bounds(),
    };
    let displays: Vec<DisplayArea> = cx.displays().iter().map(area).collect();
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
        (WindowBounds::Windowed(placed), false)
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
    let Some(bounds) = resolve_window_bounds(saved, &displays, primary.as_ref()) else {
        return fallback();
    };
    match (state.is_maximized, state.is_full_screen) {
        (true, full_screen) => (WindowBounds::Maximized(bounds), full_screen),
        (false, true) => (WindowBounds::Fullscreen(bounds), false),
        (false, false) => (WindowBounds::Windowed(bounds), false),
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

            let (window_bounds, enter_full_screen) = initial_window_bounds(cx);
            let options = WindowOptions {
                window_bounds: Some(window_bounds),
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
                if enter_full_screen {
                    window.on_next_frame(|window, _| window.toggle_fullscreen());
                }
                cx.new(|cx| BuddyApp::new(window, cx))
            })
            .expect("failed to open the Buddy window");
            cx.activate(true);
        });
}
