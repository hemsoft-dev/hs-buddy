//! Saving the window's placement to `window-state.json` in Electron's
//! `electron-window-state` format, so the next launch of either app restores
//! it (`main.rs` reads it back through `initial_window_bounds`).

use std::path::PathBuf;
use std::time::Duration;

use buddy_core::config::{DisplayRect, WindowState};
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
}

/// The window's latest placement and the last one written to disk.
pub struct WindowPersistence {
    /// `None` when no configuration directory is known; nothing is saved.
    path: Option<PathBuf>,
    /// The last rectangle of the window while neither maximized nor full
    /// screen: the restore rectangle on backends that do not report one.
    windowed: Option<DisplayRect>,
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

    /// Persistence to `path`, starting from the restore rectangle saved
    /// there, which a window launched maximized has not reported yet.
    fn at(path: Option<PathBuf>) -> Self {
        let windowed = path
            .as_deref()
            .and_then(WindowState::load_from)
            .map(|state| restore_rect(&state));
        Self {
            path,
            windowed,
            latest: None,
            saved: None,
            generation: 0,
        }
    }

    /// Note the window's current placement. Returns the generation to pass
    /// to [`save_if_current`](Self::save_if_current) after [`SAVE_DELAY`],
    /// or `None` when the placement did not change.
    pub fn record(&mut self, placement: Placement) -> Option<u64> {
        let state = capture(placement, self.windowed, REPORTS_RESTORE_BOUNDS);
        if !state.is_maximized && !state.is_full_screen {
            self.windowed = Some(restore_rect(&state));
        }
        if self.latest.as_ref() == Some(&state) {
            return None;
        }
        self.latest = Some(state);
        self.generation += 1;
        Some(self.generation)
    }

    /// The placement most recently recorded.
    pub fn latest(&self) -> Option<&WindowState> {
        self.latest.as_ref()
    }

    /// Write the placement unless a later change superseded `generation`
    /// (that change schedules its own save).
    pub fn save_if_current(&mut self, generation: u64) {
        if generation == self.generation {
            self.flush();
        }
    }

    /// Write the latest placement unless it is already on disk. A failed
    /// write is logged; the next change or quit tries again.
    pub fn flush(&mut self) {
        let (Some(path), Some(state)) = (&self.path, &self.latest) else {
            return;
        };
        if self.saved.as_ref() == Some(state) {
            return;
        }
        match state.save_to(path) {
            Ok(()) => self.saved = Some(state.clone()),
            Err(err) => log::warn!("could not save the window state: {err}"),
        }
    }
}

/// The state to save for `placement`. Like `electron-window-state`, it keeps
/// the restore rectangle of a maximized or full screen window, and keeps a
/// window maximized underneath full screen marked maximized. The reported
/// rectangle is used where it is a restore rectangle, else the last
/// `windowed` one.
fn capture(
    placement: Placement,
    windowed: Option<DisplayRect>,
    reports_restore_bounds: bool,
) -> WindowState {
    let is_maximized =
        placement.is_maximized || matches!(placement.bounds, WindowBounds::Maximized(_));
    let is_full_screen =
        placement.is_full_screen || matches!(placement.bounds, WindowBounds::Fullscreen(_));
    let reported_is_restore = match placement.bounds {
        WindowBounds::Windowed(_) => false,
        WindowBounds::Maximized(_) => reports_restore_bounds,
        // The rectangle before full screen, which on macOS and X11 is the
        // maximized one when the window was maximized.
        WindowBounds::Fullscreen(_) => reports_restore_bounds || !is_maximized,
    };
    let rect = match windowed {
        Some(windowed) if (is_maximized || is_full_screen) && !reported_is_restore => windowed,
        _ => rect(placement.bounds.get_bounds()),
    };
    WindowState {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        is_maximized,
        is_full_screen,
        display_bounds: placement.display.map(self::rect),
    }
}

/// Whether Electron's saved display (`ui.displayId`, described by
/// `electron_display`) is not the one the window is on now. Electron moves
/// its window back onto that display, so the id must go once the native
/// window has left it. Both apps measure displays in logical pixels;
/// rounding absorbs fractional scale factors.
pub fn names_another_display(electron_display: DisplayRect, current: DisplayRect) -> bool {
    let round = |r: DisplayRect| [r.x, r.y, r.width, r.height].map(|value| value.round() as i64);
    round(electron_display) != round(current)
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
        }
    }

    const WINDOWED: DisplayRect = DisplayRect {
        x: 120.0,
        y: 90.0,
        width: 1000.0,
        height: 700.0,
    };

    #[test]
    fn full_screen_over_a_maximized_window_stays_maximized() {
        let screen = bounds(0.0, 0.0, 1920.0, 1080.0);
        let zoomed = bounds(0.0, 25.0, 1920.0, 1030.0);
        // Windows: the reported rectangle is the normal one.
        let windows = capture(
            placement(
                WindowBounds::Fullscreen(bounds(120.0, 90.0, 1000.0, 700.0)),
                true,
            ),
            None,
            true,
        );
        // macOS: the rectangle before full screen is the zoomed one.
        let mac = capture(
            placement(WindowBounds::Fullscreen(zoomed), true),
            Some(WINDOWED),
            false,
        );
        // X11 reports full screen only through `is_fullscreen`.
        let x11 = capture(
            Placement {
                is_full_screen: true,
                ..placement(WindowBounds::Windowed(screen), false)
            },
            Some(WINDOWED),
            false,
        );
        for state in [&windows, &mac] {
            assert!(state.is_maximized && state.is_full_screen);
            assert_eq!(rect_of(state), (120.0, 90.0, 1000.0, 700.0));
        }
        assert!(x11.is_full_screen && !x11.is_maximized);
        assert_eq!(rect_of(&x11), (120.0, 90.0, 1000.0, 700.0));
    }

    #[test]
    fn electron_display_is_released_only_for_another_display() {
        let display = DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 1706.67,
            height: 960.0,
        };
        let same = DisplayRect {
            width: 1707.0,
            ..display
        };
        let other = DisplayRect { x: 1707.0, ..same };
        assert!(!names_another_display(display, same));
        assert!(names_another_display(display, other));
    }

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
            let state = capture(placement(window, maximized), None, true);
            assert_eq!(rect_of(&state), (120.0, 90.0, 1000.0, 700.0));
            assert_eq!(
                (state.is_maximized, state.is_full_screen),
                (maximized, full_screen)
            );
            assert_eq!(
                state.display_bounds,
                Some(DisplayRect {
                    x: 0.0,
                    y: 0.0,
                    width: 1920.0,
                    height: 1080.0
                })
            );
        }
    }

    #[test]
    fn maximized_without_a_restore_rectangle_keeps_the_last_windowed_one() {
        let windowed = Some(WINDOWED);
        let zoomed = bounds(0.0, 25.0, 1920.0, 1030.0);
        // macOS: a zoomed window is reported as `Windowed` at full size.
        let mac = capture(
            placement(WindowBounds::Windowed(zoomed), true),
            windowed,
            false,
        );
        // X11: `Maximized`, but with the maximized rectangle.
        let x11 = capture(
            placement(WindowBounds::Maximized(zoomed), true),
            windowed,
            false,
        );
        // Full screen carries a real restore rectangle on every backend.
        let full = capture(
            placement(
                WindowBounds::Fullscreen(bounds(5.0, 5.0, 900.0, 600.0)),
                false,
            ),
            windowed,
            false,
        );
        assert!(mac.is_maximized && x11.is_maximized);
        assert_eq!(rect_of(&mac), (120.0, 90.0, 1000.0, 700.0));
        assert_eq!(rect_of(&x11), (120.0, 90.0, 1000.0, 700.0));
        assert_eq!(rect_of(&full), (5.0, 5.0, 900.0, 600.0));
    }

    #[test]
    fn the_saved_rectangle_seeds_the_windowed_one() {
        let dir = std::env::temp_dir().join(format!("buddy-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("window-state.json");
        let saved = WindowState {
            x: 120.0,
            y: 90.0,
            width: 1000.0,
            height: 700.0,
            is_maximized: true,
            ..WindowState::default()
        };
        saved.save_to(&path).unwrap();
        let persistence = WindowPersistence::at(Some(path));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(persistence.windowed, Some(restore_rect(&saved)));
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
        persistence.save_if_current(stale);
        let after_stale = WindowState::load_from(&path);
        persistence.save_if_current(current);
        let after_current = WindowState::load_from(&path);
        // Nothing changed since: quitting does not rewrite the file.
        std::fs::remove_file(&path).unwrap();
        persistence.flush();
        let rewritten = path.exists();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(after_stale, None);
        assert!(
            after_current
                .as_ref()
                .is_some_and(|state| state.is_maximized)
        );
        assert_eq!(after_current, expected);
        assert!(!rewritten);
    }
}
