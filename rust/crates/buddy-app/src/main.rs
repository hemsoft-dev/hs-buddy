// A desktop app: release builds on Windows must not open a console window
// next to the GPUI window. Debug builds keep it so `RUST_LOG` output shows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod command_line;
mod dashboard;
mod github_view;
mod logging;
mod notification_audio;
mod platform_placement;
mod runtime;
mod settings;
mod settings_view;
mod shell;
mod terminal_view;
mod theme;
mod window_geometry;
mod window_persistence;
#[cfg(any(windows, test))]
mod windows_coordinates;
mod zoom;

use buddy_core::config::{AppConfig, DisplayRect, WindowState};
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
    restore_physical: Option<DisplayRect>,
}

/// Match stable IDs instead of rectangles: macOS screens can have identical
/// local bounds. A disconnected display deliberately falls back to primary.
fn saved_display_index(saved: Option<&str>, connected: &[Option<String>]) -> Option<usize> {
    let saved = saved?;
    connected
        .iter()
        .position(|uuid| uuid.as_deref() == Some(saved))
}

struct SavedDisplay {
    id: DisplayId,
    uuid: Option<String>,
    area: DisplayArea,
}

fn resolve_saved_placement(
    state: &WindowState,
    displays: &[SavedDisplay],
    primary: Option<usize>,
    screen_local: bool,
) -> Option<(Bounds<Pixels>, Option<DisplayId>)> {
    // Wayland exposes connected outputs but has no designated primary.
    let primary = primary.or_else(|| (!displays.is_empty()).then_some(0));
    let uuids: Vec<_> = displays
        .iter()
        .map(|display| display.uuid.clone())
        .collect();
    let uuid = saved_display_index(state.native_display_uuid.as_deref(), &uuids);
    let matching = |index: usize| {
        let bounds = displays[index].area.bounds;
        let Some(saved) = state.display_bounds else {
            return false;
        };
        saved.x == f64::from(f32::from(bounds.origin.x))
            && saved.y == f64::from(f32::from(bounds.origin.y))
            && saved.width == f64::from(f32::from(bounds.size.width))
            && saved.height == f64::from(f32::from(bounds.size.height))
    };
    let canonical = displays
        .iter()
        .enumerate()
        .find_map(|(index, _)| matching(index).then_some(index));
    let selected = if screen_local {
        // Electron updates global display bounds but retains unknown fields.
        // A UUID whose bounds no longer match must not override its new display.
        uuid.filter(|&index| matching(index))
            .or(canonical)
            .or(uuid)
            .or(primary)
    } else {
        None
    };
    let selected_area = selected.map(|index| displays[index].area);
    let all: Vec<_> = displays.iter().map(|display| display.area).collect();
    let candidates = if screen_local {
        selected_area.as_slice()
    } else {
        &all
    };
    let mut saved = Bounds {
        origin: point(px(state.x as f32), px(state.y as f32)),
        size: size(px(state.width as f32), px(state.height as f32)),
    };
    if screen_local
        && canonical.is_none()
        && let Some(index) = uuid
    {
        // Current records are global; retain their offset when a monitor moves.
        // Only records without display metadata use the legacy local format.
        let old_origin = state.display_bounds.map_or(point(px(0.0), px(0.0)), |old| {
            point(px(old.x as f32), px(old.y as f32))
        });
        saved.origin += displays[index].area.bounds.origin - old_origin;
    }
    let primary_area = primary.map(|index| displays[index].area);
    let mut bounds = resolve_window_bounds(saved, candidates, primary_area.as_ref())?;
    if screen_local {
        let contains = |index: usize| {
            let area = displays[index].area.bounds;
            bounds.origin.x >= area.origin.x
                && bounds.origin.y >= area.origin.y
                && bounds.origin.x + bounds.size.width <= area.origin.x + area.size.width
                && bounds.origin.y + bounds.size.height <= area.origin.y + area.size.height
        };
        let actual = selected.filter(|&index| contains(index)).or_else(|| {
            displays
                .iter()
                .enumerate()
                .find_map(|(index, _)| contains(index).then_some(index))
        });
        let index = actual?;
        bounds.origin -= displays[index].area.bounds.origin;
        return Some((bounds, Some(displays[index].id)));
    }
    Some((bounds, None))
}

fn native_window_options(placement: &InitialPlacement) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(placement.bounds),
        display_id: placement.display_id,
        window_min_size: Some(size(px(800.0), px(600.0))),
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some(env!("BUDDY_APP_ID").into()),
        titlebar: Some(TitlebarOptions {
            title: Some("Buddy".into()),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    }
}

fn initial_window_bounds(cx: &App) -> InitialPlacement {
    let area = |display: &std::rc::Rc<dyn gpui_kit::PlatformDisplay>| {
        platform_placement::display_area(display.as_ref(), cx)
    };
    let connected = cx.displays();
    let displays: Vec<SavedDisplay> = connected
        .iter()
        .map(|display| SavedDisplay {
            id: display.id(),
            uuid: display.uuid().ok().map(|id| id.to_string()),
            area: area(display),
        })
        .collect();
    let primary_display = cx.primary_display().or_else(|| connected.first().cloned());
    let primary = primary_display.as_ref().map(area);
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
            display_id: primary_display.as_ref().map(|display| display.id()),
            restore_outer: None,
            restore_physical: None,
        }
    };
    let Some(state) = WindowState::load() else {
        return fallback();
    };
    // Displays may have moved or shrunk since the state was written: fit the
    // window to the matching display's work area (`resolveWindowBounds`).
    // macOS rectangles are screen-local, and all screens have origin zero.
    // A disconnected saved UUID must resolve against the primary display,
    // rather than arbitrarily matching another screen with the same size.
    let primary_index = primary_display.as_ref().and_then(|primary| {
        displays
            .iter()
            .position(|display| display.id == primary.id())
    });
    let Some((bounds, selected_id)) =
        resolve_saved_placement(&state, &displays, primary_index, cfg!(target_os = "macos"))
    else {
        return fallback();
    };
    let (window_bounds, enter_full_screen) = match (state.is_maximized, state.is_full_screen) {
        (true, full_screen) => (WindowBounds::Maximized(bounds), full_screen),
        (false, true) if cfg!(windows) => (WindowBounds::Windowed(bounds), true),
        (false, true) => (WindowBounds::Fullscreen(bounds), false),
        (false, false) => (WindowBounds::Windowed(bounds), false),
    };
    let display_id = platform_placement::display_for_bounds(bounds, cx).or(selected_id);
    let gpui_bounds = platform_placement::bounds_for_gpui(bounds, cx);
    let window_bounds = match window_bounds {
        WindowBounds::Windowed(_) => WindowBounds::Windowed(gpui_bounds),
        WindowBounds::Maximized(_) => WindowBounds::Maximized(gpui_bounds),
        WindowBounds::Fullscreen(_) => WindowBounds::Fullscreen(gpui_bounds),
    };
    let restore_native_bounds = cfg!(windows)
        || (cfg!(target_os = "linux") && matches!(window_bounds, WindowBounds::Windowed(_)));
    InitialPlacement {
        bounds: window_bounds,
        enter_full_screen,
        display_id,
        restore_outer: restore_native_bounds.then_some(bounds),
        restore_physical: platform_placement::physical_bounds_for_state(&state, bounds, cx),
    }
}

fn open_primary_window(cx: &mut App) {
    let placement = initial_window_bounds(cx);
    let options = native_window_options(&placement);
    gpui_kit::open_window(options, cx, |window, cx| {
        if let Some(bounds) = placement.restore_outer {
            platform_placement::apply_restore_bounds(
                window,
                bounds,
                cx,
                placement.restore_physical,
            );
        }
        // Quit while the owned HWND still exists, before Windows teardown.
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
}

fn open_when_displays_ready(cx: &mut App) {
    if !cx.displays().is_empty() {
        open_primary_window(cx);
        return;
    }
    // Wayland output properties arrive after the initial registry callback.
    // Let the event loop receive them before fitting saved geometry. A host
    // with no outputs still takes the existing fallback after two seconds.
    cx.spawn(async |cx| {
        for _ in 0..100 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
            if cx.update(|cx| !cx.displays().is_empty()) {
                break;
            }
        }
        cx.update(open_primary_window);
    })
    .detach();
}

fn main() {
    match command_line::parse(std::env::args_os().skip(1)) {
        Ok(command_line::Command::Open) => {}
        Ok(command_line::Command::TerminalHost) => {
            std::process::exit(buddy_core::terminal::run_terminal_host());
        }
        Ok(command_line::Command::Help) => {
            command_line::print_help();
            return;
        }
        Ok(command_line::Command::Version) => {
            println!("{} {}", env!("BUDDY_PRODUCT_NAME"), env!("BUDDY_VERSION"));
            return;
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
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
            cx.set_global(Settings::new(config.clone()));
            cx.set_global(Runtime::start());
            zoom::init(cx);
            terminal_view::init(cx);

            cx.bind_keys([
                KeyBinding::new("secondary-q", Quit, None),
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

            open_when_displays_ready(cx);
        });
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    #[test]
    fn a_compositor_without_a_primary_output_retains_saved_size_on_recovery() {
        let state = WindowState {
            x: 50000.0,
            y: 50000.0,
            width: 900.0,
            height: 650.0,
            ..WindowState::default()
        };
        let (bounds, _) = resolve_saved_placement(&state, &[display(1, "output")], None, false)
            .expect("a connected output can recover saved geometry without a primary");
        assert_eq!(bounds.size, size(px(900.0), px(650.0)));
        assert_eq!(bounds.origin, point(px(510.0), px(215.0)));
    }

    fn display(id: u64, uuid: &str) -> SavedDisplay {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1920.0), px(1080.0)),
        };
        SavedDisplay {
            id: DisplayId::new(id),
            uuid: Some(uuid.into()),
            area: DisplayArea {
                bounds,
                work_area: Bounds {
                    origin: point(px(0.0), px(25.0)),
                    size: size(px(1920.0), px(1030.0)),
                },
            },
        }
    }

    fn startup_options(state: &WindowState, displays: &[SavedDisplay]) -> WindowOptions {
        let (bounds, id) = resolve_saved_placement(state, displays, Some(0), true).unwrap();
        native_window_options(&InitialPlacement {
            bounds: WindowBounds::Windowed(bounds),
            enter_full_screen: false,
            display_id: id,
            restore_outer: None,
            restore_physical: None,
        })
    }

    #[test]
    fn macos_startup_options_restore_secondary_display_with_identical_local_bounds() {
        let state = WindowState {
            x: 120.0,
            y: 90.0,
            width: 1000.0,
            height: 700.0,
            native_display_uuid: Some("secondary".into()),
            ..WindowState::default()
        };
        let options = startup_options(&state, &[display(1, "primary"), display(2, "secondary")]);
        assert_eq!(options.display_id, Some(DisplayId::new(2)));
        let actual = options.window_bounds.unwrap().get_bounds();
        assert_eq!(actual.origin, point(px(120.0), px(90.0)));
        assert_eq!(actual.size, size(px(1000.0), px(700.0)));
    }

    #[test]
    fn electron_macos_global_bounds_select_display_and_ignore_stale_uuid() {
        let mut secondary = display(2, "secondary");
        secondary.area.bounds.origin = point(px(1920.0), px(-240.0));
        secondary.area.work_area.origin = secondary.area.bounds.origin;
        let displays = [display(1, "primary"), secondary];
        for uuid in [None, Some("primary"), Some("secondary")] {
            let state = WindowState {
                x: 2040.0,
                y: -150.0,
                width: 1000.0,
                height: 700.0,
                display_bounds: Some(DisplayRect {
                    x: 1920.0,
                    y: -240.0,
                    width: 1920.0,
                    height: 1080.0,
                }),
                native_display_uuid: uuid.map(str::to_string),
                ..WindowState::default()
            };
            let options = startup_options(&state, &displays);
            assert_eq!(options.display_id, Some(DisplayId::new(2)));
            assert_eq!(
                options.window_bounds.unwrap().get_bounds().origin,
                point(px(120.0), px(90.0))
            );
        }
    }

    #[test]
    fn disconnected_saved_uuid_and_offscreen_position_resolve_to_primary_options() {
        let state = WindowState {
            x: 20000.0,
            y: 90.0,
            width: 1000.0,
            height: 700.0,
            native_display_uuid: Some("disconnected".into()),
            ..WindowState::default()
        };
        let options = startup_options(&state, &[display(1, "primary")]);
        assert_eq!(options.display_id, Some(DisplayId::new(1)));
        assert_eq!(
            options.window_bounds.unwrap().get_bounds().origin,
            point(px(460.0), px(190.0))
        );
    }

    #[test]
    fn macos_global_record_retains_offset_when_secondary_moves_to_the_left() {
        let mut secondary = display(2, "secondary");
        secondary.area.bounds.origin = point(px(-1920.0), px(-240.0));
        secondary.area.work_area.origin = secondary.area.bounds.origin;
        let state = WindowState {
            x: 2040.0,
            y: -150.0,
            width: 1000.0,
            height: 700.0,
            display_bounds: Some(buddy_core::config::DisplayRect {
                x: 1920.0,
                y: -240.0,
                width: 1920.0,
                height: 1080.0,
            }),
            native_display_uuid: Some("secondary".into()),
            ..WindowState::default()
        };
        let options = startup_options(&state, &[display(1, "primary"), secondary]);
        assert_eq!(options.display_id, Some(DisplayId::new(2)));
        assert_eq!(
            options.window_bounds.unwrap().get_bounds().origin,
            point(px(120.0), px(90.0))
        );
    }

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
