//! Native placement details that GPUI's portable bounds do not expose.

use crate::window_geometry::DisplayArea;
use buddy_core::config::{DisplayRect, NativeWindowGeometry, WindowState};
use gpui_kit::{App, Bounds, DisplayId, Pixels, PlatformDisplay, Window};

pub fn display_area(display: &dyn PlatformDisplay, cx: &App) -> DisplayArea {
    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        macos::display_area(display)
    }
    #[cfg(windows)]
    if let Some(area) = windows::display_area(display.id(), cx) {
        return area;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = cx;
    #[cfg(not(target_os = "macos"))]
    DisplayArea {
        bounds: display.bounds(),
        work_area: display.visible_bounds(),
    }
}

pub fn display_for_bounds(bounds: Bounds<Pixels>, cx: &App) -> Option<DisplayId> {
    #[cfg(windows)]
    {
        windows::matching_monitor(bounds, cx).map(|m| DisplayId::new(m.id))
    }
    #[cfg(not(windows))]
    {
        let _ = (bounds, cx);
        None
    }
}

pub fn bounds_for_gpui(bounds: Bounds<Pixels>, cx: &App) -> Bounds<Pixels> {
    #[cfg(windows)]
    if let Some(monitor) = windows::matching_monitor(bounds, cx) {
        let mut physical = monitor.to_physical(windows::rect(bounds));
        physical.x /= monitor.scale;
        physical.y /= monitor.scale;
        physical.width /= monitor.scale;
        physical.height /= monitor.scale;
        return windows::bounds(physical);
    }
    #[cfg(not(windows))]
    let _ = cx;
    bounds
}

pub fn native_geometry(window: &Window, cx: &App) -> Option<NativeWindowGeometry> {
    #[cfg(windows)]
    {
        windows::native_geometry(window, cx)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, cx);
        None
    }
}

pub fn physical_bounds_for_state(
    state: &WindowState,
    resolved: Bounds<Pixels>,
    cx: &App,
) -> Option<DisplayRect> {
    #[cfg(windows)]
    {
        let geometry = state.native_geometry?;
        if !geometry.matches(state) {
            return None;
        }
        let actual = windows::rect(resolved);
        let round = |r: DisplayRect| [r.x, r.y, r.width, r.height].map(f64::round);
        let monitor = windows::matching_monitor(resolved, cx)?;
        (round(actual) == round(geometry.logical) && geometry_matches_monitor(geometry, monitor))
            .then_some(geometry.physical)
    }
    #[cfg(not(windows))]
    {
        let _ = (state, resolved, cx);
        None
    }
}

#[cfg(any(windows, test))]
fn geometry_matches_monitor(
    geometry: NativeWindowGeometry,
    monitor: crate::windows_coordinates::Monitor,
) -> bool {
    let r = geometry.physical;
    let d = monitor.physical;
    d == geometry.physical_display_bounds
        && monitor.dip == geometry.display_bounds
        && r.x >= d.x - 64.0
        && r.y >= d.y - 64.0
        && r.x + r.width <= d.x + d.width + 64.0
        && r.y + r.height <= d.y + d.height + 64.0
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn exact_geometry_requires_the_current_display_scale() {
        let display = DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let window = DisplayRect {
            x: 120.0,
            y: 90.0,
            width: 1000.0,
            height: 700.0,
        };
        let geometry = NativeWindowGeometry {
            logical: window,
            physical: window,
            display_bounds: display,
            physical_display_bounds: display,
        };
        let monitor = crate::windows_coordinates::Monitor::new(1, display, display, 1.0);
        assert!(geometry_matches_monitor(geometry, monitor));
        let scaled = crate::windows_coordinates::Monitor::new(1, display, display, 1.5);
        assert!(!geometry_matches_monitor(geometry, scaled));
    }
}

/// Skip minimized windows, including X11 windows still mapped by the WM.
pub fn can_capture(window: &Window) -> bool {
    if !window.is_visible() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let id = match handle.as_raw() {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => handle.window as u32,
            _ => return true,
        };
        // Failure must preserve the last known placement, rather than erase
        // maximization using a snapshot whose minimized state is unknown.
        match x11_is_hidden(id) {
            Ok(hidden) => !hidden,
            Err(err) => {
                log::warn!("could not read X11 window state; preserving saved placement: {err}");
                false
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    true
}

#[cfg(target_os = "linux")]
fn x11_is_hidden(id: u32) -> Result<bool, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    type Query = (x11rb::rust_connection::RustConnection, u32, u32);
    thread_local! {
        static QUERY: std::cell::OnceCell<Result<Query, String>> = const { std::cell::OnceCell::new() };
    }
    // Bounds notifications share one connection and intern each atom once.
    // Only the window's property changes during a move/resize operation.
    QUERY.with(|cached| {
        let query = cached.get_or_init(|| {
            let connect = || -> Result<Query, Box<dyn std::error::Error>> {
                let (connection, _) = x11rb::connect(None)?;
                let state = connection
                    .intern_atom(false, b"_NET_WM_STATE")?
                    .reply()?
                    .atom;
                let hidden = connection
                    .intern_atom(false, b"_NET_WM_STATE_HIDDEN")?
                    .reply()?
                    .atom;
                Ok((connection, state, hidden))
            };
            connect().map_err(|err| err.to_string())
        });
        let (connection, state, hidden) = query.as_ref().map_err(|err| err.clone())?;
        let reply = connection
            .get_property(false, id, *state, AtomEnum::ATOM, 0, u32::MAX)?
            .reply()?;
        Ok(reply
            .value32()
            .is_some_and(|mut values| values.any(|value| value == *hidden)))
    })
}

/// Windows exposes outer restore bounds in workspace coordinates. Electron
/// stores outer screen DIPs. Read the OS rectangle instead of GPUI's client
/// rectangle, accounting for a taskbar on the top or left of the monitor.
pub fn restore_bounds(window: &Window, cx: &App) -> Option<Bounds<Pixels>> {
    #[cfg(windows)]
    {
        windows::restore_bounds(window, cx)
    }
    #[cfg(target_os = "macos")]
    {
        let mut bounds = window.window_bounds().get_bounds();
        let display = window.display(cx)?;
        bounds.origin += display_area(display.as_ref(), cx).bounds.origin;
        Some(bounds)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = cx;
        Some(window.window_bounds().get_bounds())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use gpui_kit::{point, px, size};

    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayBounds(display: u32) -> Rect;
    }

    pub(super) fn display_area(display: &dyn PlatformDisplay) -> DisplayArea {
        // GPUI deliberately zeroes this origin. Electron uses the original
        // CoreGraphics top-left global coordinates instead.
        let rect = unsafe { CGDisplayBounds(u64::from(display.id()) as u32) };
        let origin = point(px(rect.origin.x as f32), px(rect.origin.y as f32));
        let bounds = Bounds {
            origin,
            size: size(px(rect.size.width as f32), px(rect.size.height as f32)),
        };
        let mut work_area = display.visible_bounds();
        work_area.origin += origin;
        DisplayArea { bounds, work_area }
    }
}

/// Apply Electron's exact outer rectangle after GPUI creates its client-sized
/// window. This uses OS placement APIs so private GPUI frame metrics never
/// need to be duplicated.
pub fn apply_restore_bounds(
    window: &Window,
    bounds: Bounds<Pixels>,
    cx: &App,
    physical: Option<DisplayRect>,
) {
    #[cfg(windows)]
    if let Err(err) = windows::apply_restore_bounds(window, bounds, cx, physical) {
        log::warn!("could not restore outer window rectangle: {err}");
    }
    #[cfg(not(windows))]
    let _ = (window, bounds, cx, physical);
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::windows_coordinates::{self, Monitor};
    use gpui_kit::{point, px, size};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI},
        UI::WindowsAndMessaging::{
            GetWindowPlacement, GetWindowRect, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME,
            SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
        },
    };

    pub fn rect(bounds: Bounds<Pixels>) -> DisplayRect {
        DisplayRect {
            x: f32::from(bounds.origin.x).into(),
            y: f32::from(bounds.origin.y).into(),
            width: f32::from(bounds.size.width).into(),
            height: f32::from(bounds.size.height).into(),
        }
    }

    pub fn bounds(rect: DisplayRect) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(rect.x as f32), px(rect.y as f32)),
            size: size(px(rect.width as f32), px(rect.height as f32)),
        }
    }

    fn physical_rect(rect: RECT) -> DisplayRect {
        DisplayRect {
            x: rect.left.into(),
            y: rect.top.into(),
            width: (rect.right - rect.left).into(),
            height: (rect.bottom - rect.top).into(),
        }
    }

    fn monitors(cx: &App) -> Vec<Monitor> {
        let mut monitors: Vec<_> = cx
            .displays()
            .iter()
            .filter_map(|display| {
                let id = u64::from(display.id());
                let handle = id as windows_sys::Win32::Graphics::Gdi::HMONITOR;
                // GPUI Kit 0.7.1 uses the HMONITOR as its Windows DisplayId.
                // SAFETY: current connected handles and initialized OS outputs.
                unsafe {
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..std::mem::zeroed()
                    };
                    let (mut x, mut y) = (0, 0);
                    if GetMonitorInfoW(handle, &mut info) == 0
                        || GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) < 0
                        || x == 0
                    {
                        return None;
                    }
                    Some(Monitor::new(
                        id,
                        physical_rect(info.rcMonitor),
                        physical_rect(info.rcWork),
                        f64::from(x) / 96.0,
                    ))
                }
            })
            .collect();
        windows_coordinates::layout(&mut monitors);
        monitors
    }

    pub fn matching_monitor(bounds: Bounds<Pixels>, cx: &App) -> Option<Monitor> {
        windows_coordinates::matching(&monitors(cx), rect(bounds))
    }

    pub fn display_area(id: DisplayId, cx: &App) -> Option<DisplayArea> {
        let monitor = monitors(cx).into_iter().find(|m| m.id == u64::from(id))?;
        Some(DisplayArea {
            bounds: bounds(monitor.dip),
            work_area: bounds(monitor.dip_work()),
        })
    }

    // BrowserWindow's frameless Windows bounds exclude the invisible resize
    // frame on the left, right and bottom. GetWindowRect includes it. Keep
    // that convention separate from GPUI's own client-area frame metrics.
    fn electron_frame(scale: f64) -> (f64, f64) {
        let dpi = (scale * 96.0).round() as u32;
        // SAFETY: reading system metrics for a verified monitor DPI.
        unsafe {
            let padding = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            (
                (f64::from(GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + padding) / scale).floor(),
                (f64::from(GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + padding) / scale).floor(),
            )
        }
    }

    fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    fn workspace_offset(hwnd: HWND) -> Option<(i32, i32)> {
        // SAFETY: the borrowed window handle remains alive throughout these
        // calls; each OS output struct has its required size initialized.
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..std::mem::zeroed()
            };
            (GetMonitorInfoW(monitor, &mut info) != 0).then_some((
                info.rcWork.left - info.rcMonitor.left,
                info.rcWork.top - info.rcMonitor.top,
            ))
        }
    }

    fn translate(rect: RECT, dx: i32, dy: i32) -> RECT {
        RECT {
            left: rect.left + dx,
            right: rect.right + dx,
            top: rect.top + dy,
            bottom: rect.bottom + dy,
        }
    }

    fn restore_rect(window: &Window, cx: &App) -> Option<(Monitor, DisplayRect)> {
        let hwnd = hwnd(window)?;
        // SAFETY: the live window owns this monitor handle.
        let handle = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let monitor = monitors(cx).into_iter().find(|m| m.id == handle as u64)?;
        let rect = unsafe {
            // SAFETY: live HWND and initialized output buffers.
            let mut rect: RECT = std::mem::zeroed();
            if window.is_maximized() {
                let mut placement = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..std::mem::zeroed()
                };
                if GetWindowPlacement(hwnd, &mut placement) == 0 {
                    return None;
                }
                rect = placement.rcNormalPosition;
                let (dx, dy) = workspace_offset(hwnd)?;
                rect = translate(rect, dx, dy);
            } else if GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
            rect
        };
        Some((monitor, physical_rect(rect)))
    }

    pub fn restore_bounds(window: &Window, cx: &App) -> Option<Bounds<Pixels>> {
        let (monitor, physical) = restore_rect(window, cx)?;
        let mut logical = monitor.to_dip(physical);
        let (horizontal, bottom) = electron_frame(monitor.scale);
        logical.x = logical.x.floor() + horizontal;
        logical.y = logical.y.floor();
        logical.width = ((physical.x + physical.width) / monitor.scale).ceil()
            - (physical.x / monitor.scale).floor()
            - 2.0 * horizontal;
        logical.height = ((physical.y + physical.height) / monitor.scale).ceil()
            - (physical.y / monitor.scale).floor()
            - bottom;
        Some(bounds(logical))
    }

    pub fn native_geometry(window: &Window, cx: &App) -> Option<NativeWindowGeometry> {
        let (monitor, physical) = restore_rect(window, cx)?;
        let mut logical = rect(restore_bounds(window, cx)?);
        logical.x = logical.x.round();
        logical.y = logical.y.round();
        logical.width = logical.width.round();
        logical.height = logical.height.round();
        Some(NativeWindowGeometry {
            logical,
            physical,
            display_bounds: monitor.dip,
            physical_display_bounds: monitor.physical,
        })
    }

    pub fn apply_restore_bounds(
        window: &Window,
        saved: Bounds<Pixels>,
        cx: &App,
        exact_physical: Option<DisplayRect>,
    ) -> std::io::Result<()> {
        let hwnd = hwnd(window).ok_or_else(|| std::io::Error::other("no Win32 window handle"))?;
        let monitor = matching_monitor(saved, cx)
            .ok_or_else(|| std::io::Error::other("saved display is unavailable"))?;
        let mut logical = rect(saved);
        let (horizontal, bottom) = electron_frame(monitor.scale);
        logical.x -= horizontal;
        logical.width += 2.0 * horizontal;
        logical.height += bottom;
        let physical = exact_physical.unwrap_or_else(|| monitor.to_physical(logical));
        let rect = RECT {
            left: physical.x.round() as i32,
            top: physical.y.round() as i32,
            right: physical.x.round() as i32 + physical.width.round() as i32,
            bottom: physical.y.round() as i32 + physical.height.round() as i32,
        };
        // SAFETY: live HWND, initialized placement and no retained pointers.
        let success = unsafe {
            if window.is_maximized() {
                let mut placement = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..std::mem::zeroed()
                };
                if GetWindowPlacement(hwnd, &mut placement) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let (dx, dy) = workspace_offset(hwnd).ok_or_else(std::io::Error::last_os_error)?;
                placement.rcNormalPosition = translate(rect, -dx, -dy);
                SetWindowPlacement(hwnd, &placement)
            } else {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            }
        };
        if success == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn values(rect: RECT) -> [i32; 4] {
            [rect.left, rect.top, rect.right, rect.bottom]
        }

        #[test]
        fn taskbars_at_bottom_top_and_left_preserve_the_screen_rectangle() {
            let screen = RECT {
                left: 120,
                top: 90,
                right: 1620,
                bottom: 1140,
            };
            // Explicit Windows workspace fixtures, not values obtained from
            // the inverse helper under test. All restore to the same outer
            // physical rectangle at 150% DPI.
            let cases = [
                (
                    RECT {
                        left: 120,
                        top: 90,
                        right: 1620,
                        bottom: 1140,
                    },
                    0,
                    0,
                ),
                (
                    RECT {
                        left: 120,
                        top: 30,
                        right: 1620,
                        bottom: 1080,
                    },
                    0,
                    60,
                ),
                (
                    RECT {
                        left: 60,
                        top: 90,
                        right: 1560,
                        bottom: 1140,
                    },
                    60,
                    0,
                ),
                (
                    RECT {
                        left: -1800,
                        top: 90,
                        right: -300,
                        bottom: 1140,
                    },
                    0,
                    0,
                ),
            ];
            for (workspace, dx, dy) in cases {
                let expected = if workspace.left < 0 {
                    [-1800, 90, -300, 1140]
                } else {
                    values(screen)
                };
                let captured = translate(workspace, dx, dy);
                assert_eq!(values(captured), expected);
                assert_eq!(values(translate(captured, -dx, -dy)), values(workspace));
            }
        }
    }
}
