//! Saving the window's placement to `window-state.json` in Electron's
//! `electron-window-state` format, so the next launch of either app restores
//! it (`main.rs` reads it back through `initial_window_bounds`).

use std::path::PathBuf;
use std::time::Duration;

use buddy_core::config::{DisplayRect, NativeWindowGeometry, WindowState};
use gpui_kit::{Bounds, Pixels, WindowBounds};

/// How long the window must stay put before a move or resize is written, so
/// a drag writes once and a killed process still restores its last placement.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// Only the Windows backend reports a maximized window's restore rectangle;
/// macOS and X11 report the maximized rectangle itself.
const REPORTS_RESTORE_BOUNDS: bool = cfg!(windows);

/// What GPUI reports about the window at one moment.
pub struct Placement {
    pub bounds: WindowBounds,
    /// macOS reports a zoomed window as `Windowed`, so the variant alone
    /// does not tell.
    pub is_maximized: bool,
    /// X11 never reports `Fullscreen` bounds.
    pub is_full_screen: bool,
    pub display: Option<Bounds<Pixels>>,
    pub native_display_uuid: Option<String>,
    pub native_geometry: Option<NativeWindowGeometry>,
}

/// A rectangle the window had while neither maximized nor full screen, and
/// the display it was on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Windowed {
    rect: DisplayRect,
    display: Option<DisplayRect>,
    native_geometry: Option<NativeWindowGeometry>,
}

/// What some backends stop reporting once the window is maximized or full
/// screen, remembered from earlier placements.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Remembered {
    /// The restore rectangle on backends that do not report one.
    windowed: Option<Windowed>,
    /// Whether the window was maximized when last seen outside full screen:
    /// Windows and macOS report a full screen window as not maximized.
    maximized: bool,
}

/// The window's latest placement and the last one written to disk.
pub struct WindowPersistence {
    /// `None` when no configuration directory is known; nothing is saved.
    path: Option<PathBuf>,
    remembered: Remembered,
    latest: Option<WindowState>,
    saved: Option<WindowState>,
    generation: u64,
}

impl WindowPersistence {
    pub fn new() -> Self {
        let path = WindowState::path()
            .inspect_err(|err| log::warn!("window state will not be saved: {err}"))
            .ok();
        Self::at(path)
    }

    /// Persistence to `path`, starting from what was saved there, which a
    /// window launched maximized or full screen has not reported yet.
    fn at(path: Option<PathBuf>) -> Self {
        let remembered = path
            .as_deref()
            .and_then(WindowState::load_from)
            .map(|state| Remembered {
                windowed: Some(Windowed {
                    rect: restore_rect(&state),
                    display: state.display_bounds,
                    native_geometry: state.native_geometry,
                }),
                maximized: state.is_maximized,
            })
            .unwrap_or_default();
        Self {
            path,
            remembered,
            latest: None,
            saved: None,
            generation: 0,
        }
    }

    /// Note the window's current placement. Returns the generation to pass
    /// to [`save_if_current`](Self::save_if_current) after [`SAVE_DELAY`],
    /// or `None` when the placement did not change.
    pub fn record(&mut self, placement: Placement) -> Option<u64> {
        let state = capture(placement, self.remembered, REPORTS_RESTORE_BOUNDS);
        if !state.is_full_screen {
            self.remembered.maximized = state.is_maximized;
        }
        if !state.is_maximized && !state.is_full_screen {
            self.remembered.windowed = Some(Windowed {
                rect: restore_rect(&state),
                display: state.display_bounds,
                native_geometry: state.native_geometry,
            });
        }
        if self.latest.as_ref() == Some(&state) {
            return None;
        }
        self.latest = Some(state);
        self.generation += 1;
        Some(self.generation)
    }

    /// The placement most recently recorded.
    #[cfg(test)]
    pub fn latest(&self) -> Option<&WindowState> {
        self.latest.as_ref()
    }

    /// Write the placement unless a later change superseded `generation`
    /// (that change schedules its own save). `true` once the latest
    /// placement is on disk.
    pub fn save_if_current(&mut self, generation: u64) -> bool {
        generation == self.generation && self.flush()
    }

    /// Write the latest placement unless it is already on disk. `true` once
    /// it is; a failed write is logged and the next change or quit tries
    /// again.
    pub fn flush(&mut self) -> bool {
        let (Some(path), Some(state)) = (&self.path, &self.latest) else {
            return false;
        };
        if self.saved.as_ref() == Some(state) {
            return true;
        }
        match state.save_to(path) {
            Ok(()) => {
                self.saved = Some(state.clone());
                true
            }
            Err(err) => {
                log::warn!("could not save the window state: {err}");
                false
            }
        }
    }
}

/// The state to save for `placement`. Like `electron-window-state`, it keeps
/// the restore rectangle of a maximized or full screen window, and keeps a
/// window maximized underneath full screen marked maximized. The reported
/// rectangle is used where it is a restore rectangle, else the remembered
/// windowed one, moved along if the window has changed display since.
fn capture(
    placement: Placement,
    remembered: Remembered,
    reports_restore_bounds: bool,
) -> WindowState {
    let is_full_screen =
        placement.is_full_screen || matches!(placement.bounds, WindowBounds::Fullscreen(_));
    let is_maximized = placement.is_maximized
        || matches!(placement.bounds, WindowBounds::Maximized(_))
        || (is_full_screen && remembered.maximized);
    let reported_is_restore = match placement.bounds {
        WindowBounds::Windowed(_) => false,
        WindowBounds::Maximized(_) => reports_restore_bounds,
        // The rectangle before full screen, which on macOS and X11 is the
        // maximized one when the window was maximized.
        WindowBounds::Fullscreen(_) => !reports_restore_bounds && !is_maximized,
    };
    let display = placement.display.map(rect);
    let (rect, native_geometry) = match remembered.windowed {
        Some(windowed) if (is_maximized || is_full_screen) && !reported_is_restore => {
            (relocate(windowed, display), windowed.native_geometry)
        }
        _ => (
            rect(placement.bounds.get_bounds()),
            placement.native_geometry,
        ),
    };
    let mut state = WindowState {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        is_maximized,
        is_full_screen,
        display_bounds: display,
        native_display_uuid: placement.native_display_uuid,
        native_geometry,
    };
    state.native_geometry = state
        .native_geometry
        .filter(|geometry| geometry.matches(&state));
    state
}

/// `windowed`'s rectangle at the same offset on `display`, when the window
/// has moved there since (a maximized window sent to another monitor).
fn relocate(windowed: Windowed, display: Option<DisplayRect>) -> DisplayRect {
    match (windowed.display, display) {
        (Some(from), Some(to)) if different_displays(from, to) => DisplayRect {
            x: windowed.rect.x - from.x + to.x,
            y: windowed.rect.y - from.y + to.y,
            ..windowed.rect
        },
        _ => windowed.rect,
    }
}

/// Whether two display rectangles describe different displays. Both apps
/// measure displays in logical pixels; rounding absorbs fractional scale
/// factors.
pub fn different_displays(a: DisplayRect, b: DisplayRect) -> bool {
    let round = |r: DisplayRect| [r.x, r.y, r.width, r.height].map(|value| value.round() as i64);
    round(a) != round(b)
}

fn restore_rect(state: &WindowState) -> DisplayRect {
    DisplayRect {
        x: state.x,
        y: state.y,
        width: state.width,
        height: state.height,
    }
}

fn rect(bounds: Bounds<Pixels>) -> DisplayRect {
    DisplayRect {
        x: f32::from(bounds.origin.x).into(),
        y: f32::from(bounds.origin.y).into(),
        width: f32::from(bounds.size.width).into(),
        height: f32::from(bounds.size.height).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        }
    }

    fn placement(bounds: WindowBounds, is_maximized: bool) -> Placement {
        Placement {
            bounds,
            is_maximized,
            is_full_screen: false,
            display: Some(self::bounds(0.0, 0.0, 1920.0, 1080.0)),
            native_display_uuid: None,
            native_geometry: None,
        }
    }

    const PRIMARY: DisplayRect = DisplayRect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    const WINDOWED: Remembered = Remembered {
        windowed: Some(Windowed {
            rect: DisplayRect {
                x: 120.0,
                y: 90.0,
                width: 1000.0,
                height: 700.0,
            },
            display: Some(PRIMARY),
            native_geometry: None,
        }),
        maximized: false,
    };

    fn rect_of(state: &WindowState) -> (f64, f64, f64, f64) {
        (state.x, state.y, state.width, state.height)
    }

    #[test]
    fn capture_keeps_the_reported_restore_rectangle_and_the_mode() {
        let restore = bounds(120.0, 90.0, 1000.0, 700.0);
        let cases = [
            (WindowBounds::Windowed(restore), false, false),
            (WindowBounds::Maximized(restore), true, false),
            (WindowBounds::Fullscreen(restore), false, true),
        ];
        for (window, maximized, full_screen) in cases {
            let state = capture(placement(window, maximized), Remembered::default(), true);
            assert_eq!(rect_of(&state), (120.0, 90.0, 1000.0, 700.0));
            assert_eq!(
                (state.is_maximized, state.is_full_screen),
                (maximized, full_screen)
            );
            assert_eq!(state.display_bounds, Some(PRIMARY));
        }
    }

    #[test]
    fn maximized_without_a_restore_rectangle_keeps_the_last_windowed_one() {
        let zoomed = bounds(0.0, 25.0, 1920.0, 1030.0);
        // macOS: a zoomed window is reported as `Windowed` at full size.
        let mac = capture(
            placement(WindowBounds::Windowed(zoomed), true),
            WINDOWED,
            false,
        );
        // X11: `Maximized`, but with the maximized rectangle.
        let x11 = capture(
            placement(WindowBounds::Maximized(zoomed), true),
            WINDOWED,
            false,
        );
        // Full screen from a normal window carries a real restore rectangle.
        let full = capture(
            placement(
                WindowBounds::Fullscreen(bounds(5.0, 5.0, 900.0, 600.0)),
                false,
            ),
            WINDOWED,
            false,
        );
        assert!(mac.is_maximized && x11.is_maximized);
        assert_eq!(rect_of(&mac), (120.0, 90.0, 1000.0, 700.0));
        assert_eq!(rect_of(&x11), (120.0, 90.0, 1000.0, 700.0));
        assert_eq!(rect_of(&full), (5.0, 5.0, 900.0, 600.0));
    }

    #[test]
    fn full_screen_over_a_maximized_window_stays_maximized() {
        let screen = bounds(0.0, 0.0, 1920.0, 1080.0);
        let zoomed = bounds(0.0, 25.0, 1920.0, 1030.0);
        let maximized = Remembered {
            maximized: true,
            ..WINDOWED
        };
        // Windows: the reported rectangle is the normal one, but
        // `is_maximized()` is false in full screen.
        let windows = capture(
            placement(
                WindowBounds::Fullscreen(bounds(120.0, 90.0, 1000.0, 700.0)),
                false,
            ),
            maximized,
            true,
        );
        // macOS: the rectangle before full screen is the zoomed one.
        let mac = capture(
            placement(WindowBounds::Fullscreen(zoomed), false),
            maximized,
            false,
        );
        // X11 reports full screen only through `is_fullscreen`.
        let x11 = capture(
            Placement {
                is_full_screen: true,
                ..placement(WindowBounds::Windowed(screen), false)
            },
            maximized,
            false,
        );
        for state in [&windows, &mac, &x11] {
            assert!(state.is_maximized && state.is_full_screen);
            assert_eq!(rect_of(state), (120.0, 90.0, 1000.0, 700.0));
        }
    }

    #[test]
    fn full_screen_remembers_the_maximized_state_it_was_entered_from() {
        let mut persistence = WindowPersistence::at(None);
        let restore = bounds(120.0, 90.0, 1000.0, 700.0);
        persistence.record(placement(WindowBounds::Maximized(restore), true));
        persistence.record(placement(WindowBounds::Fullscreen(restore), false));
        let over_maximized = persistence.latest().cloned().unwrap();
        persistence.record(placement(WindowBounds::Windowed(restore), false));
        persistence.record(placement(WindowBounds::Fullscreen(restore), false));
        let over_windowed = persistence.latest().cloned().unwrap();
        assert!(over_maximized.is_maximized && over_maximized.is_full_screen);
        assert!(!over_windowed.is_maximized && over_windowed.is_full_screen);
    }

    #[test]
    fn a_maximized_window_moved_to_another_display_takes_its_restore_rectangle_along() {
        let right = Some(bounds(1920.0, 0.0, 2560.0, 1440.0));
        let state = capture(
            Placement {
                display: right,
                ..placement(
                    WindowBounds::Maximized(bounds(1920.0, 0.0, 2560.0, 1400.0)),
                    true,
                )
            },
            WINDOWED,
            false,
        );
        assert_eq!(rect_of(&state), (2040.0, 90.0, 1000.0, 700.0));
        assert_eq!(state.display_bounds.map(|d| d.x), Some(1920.0));
    }

    #[test]
    fn displays_are_compared_by_rounded_bounds() {
        let display = DisplayRect {
            width: 1706.67,
            ..PRIMARY
        };
        let same = DisplayRect {
            width: 1707.0,
            ..PRIMARY
        };
        let other = DisplayRect { x: 1707.0, ..same };
        assert!(!different_displays(display, same));
        assert!(different_displays(display, other));
    }

    #[test]
    fn the_saved_state_seeds_what_backends_stop_reporting() {
        let dir = std::env::temp_dir().join(format!("buddy-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("window-state.json");
        let saved = WindowState {
            x: 120.0,
            y: 90.0,
            width: 1000.0,
            height: 700.0,
            is_maximized: true,
            is_full_screen: true,
            display_bounds: Some(PRIMARY),
            native_display_uuid: None,
            native_geometry: None,
        };
        saved.save_to(&path).unwrap();
        let persistence = WindowPersistence::at(Some(path));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            persistence.remembered,
            Remembered {
                maximized: true,
                ..WINDOWED
            }
        );
    }

    #[test]
    fn only_the_latest_unchanged_placement_is_written() {
        let dir = std::env::temp_dir().join(format!("buddy-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("window-state.json");
        let mut persistence = WindowPersistence::at(Some(path.clone()));
        let first = || {
            placement(
                WindowBounds::Windowed(bounds(0.0, 0.0, 800.0, 600.0)),
                false,
            )
        };
        let second = placement(
            WindowBounds::Maximized(bounds(10.0, 10.0, 900.0, 700.0)),
            true,
        );

        let stale = persistence.record(first()).unwrap();
        assert_eq!(persistence.record(first()), None, "unchanged placement");
        let current = persistence.record(second).unwrap();
        let expected = persistence.latest.clone();
        // The first move's timer fires after the second move: nothing yet.
        let saved_stale = persistence.save_if_current(stale);
        let after_stale = WindowState::load_from(&path);
        let saved_current = persistence.save_if_current(current);
        let after_current = WindowState::load_from(&path);
        // Nothing changed since: quitting does not rewrite the file.
        std::fs::remove_file(&path).unwrap();
        let flushed = persistence.flush();
        let rewritten = path.exists();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!saved_stale);
        assert_eq!(after_stale, None);
        assert!(saved_current);
        assert!(
            after_current
                .as_ref()
                .is_some_and(|state| state.is_maximized)
        );
        assert_eq!(after_current, expected);
        assert!(flushed);
        assert!(!rewritten);
    }

    #[test]
    fn a_failed_write_is_not_reported_as_saved() {
        let dir = std::env::temp_dir().join(format!("buddy-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A directory where the file should be makes the rename fail.
        let path = dir.join("window-state.json");
        std::fs::create_dir_all(&path).unwrap();
        let mut persistence = WindowPersistence::at(Some(path));
        let generation = persistence
            .record(placement(
                WindowBounds::Windowed(bounds(0.0, 0.0, 800.0, 600.0)),
                false,
            ))
            .unwrap();
        let saved = persistence.save_if_current(generation);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!saved);
    }
}
