//! Native placement details that GPUI's portable bounds do not expose.

use gpui_kit::{Bounds, Pixels, Window};

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
    let (connection, _) = x11rb::connect(None)?;
    let state = connection
        .intern_atom(false, b"_NET_WM_STATE")?
        .reply()?
        .atom;
    let hidden = connection
        .intern_atom(false, b"_NET_WM_STATE_HIDDEN")?
        .reply()?
        .atom;
    let reply = connection
        .get_property(false, id, state, AtomEnum::ATOM, 0, u32::MAX)?
        .reply()?;
    Ok(reply
        .value32()
        .is_some_and(|mut values| values.any(|value| value == hidden)))
}

/// Windows exposes outer restore bounds in workspace coordinates. Electron
/// stores outer screen DIPs. Read the OS rectangle instead of GPUI's client
/// rectangle, accounting for a taskbar on the top or left of the monitor.
pub fn restore_bounds(window: &Window) -> Option<Bounds<Pixels>> {
    #[cfg(windows)]
    {
        windows::restore_bounds(window)
    }
    #[cfg(not(windows))]
    {
        Some(window.window_bounds().get_bounds())
    }
}

/// Apply Electron's exact outer rectangle after GPUI creates its client-sized
/// window. This uses OS placement APIs so private GPUI frame metrics never
/// need to be duplicated.
pub fn apply_restore_bounds(window: &Window, bounds: Bounds<Pixels>) {
    #[cfg(windows)]
    if let Err(err) = windows::apply_restore_bounds(window, bounds) {
        log::warn!("could not restore outer window rectangle: {err}");
    }
    #[cfg(not(windows))]
    let _ = (window, bounds);
}

#[cfg(windows)]
mod windows {
    use super::*;
    use gpui_kit::{point, px, size};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::{
            GetWindowPlacement, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement,
            SetWindowPos, WINDOWPLACEMENT,
        },
    };

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

    pub fn restore_bounds(window: &Window) -> Option<Bounds<Pixels>> {
        let hwnd = hwnd(window)?;
        let scale = window.scale_factor();
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
        Some(Bounds {
            origin: point(px(rect.left as f32 / scale), px(rect.top as f32 / scale)),
            size: size(
                px((rect.right - rect.left) as f32 / scale),
                px((rect.bottom - rect.top) as f32 / scale),
            ),
        })
    }

    pub fn apply_restore_bounds(window: &Window, bounds: Bounds<Pixels>) -> std::io::Result<()> {
        let hwnd = hwnd(window).ok_or_else(|| std::io::Error::other("no Win32 window handle"))?;
        let scale = window.scale_factor();
        let physical = |value: Pixels| (f32::from(value) * scale).round() as i32;
        let rect = RECT {
            left: physical(bounds.origin.x),
            top: physical(bounds.origin.y),
            right: physical(bounds.origin.x) + physical(bounds.size.width),
            bottom: physical(bounds.origin.y) + physical(bounds.size.height),
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
