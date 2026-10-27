//! WebKitGTK inside GPUI's X11 window. wry wants an Xlib parent and an
//! initialized GTK pumped by the host app; GPUI hands out XCB and has no
//! GTK. The `gtk` crate itself isn't a dependency here: its bindings are
//! flagged unmaintained, so this module talks to the three GTK/GDK calls it
//! needs directly and lets tao (wry's maintained sibling) set gtk-rs's own
//! "initialized" flag, which webkit2gtk checks.

use std::cell::Cell;
use std::os::raw::{c_char, c_int, c_ulong};

use raw_window_handle::{RawWindowHandle, XlibWindowHandle};

use super::host::{Unavailable, no_window};

#[link(name = "gdk-3")]
unsafe extern "C" {
    fn gdk_set_allowed_backends(backends: *const c_char);
}

#[link(name = "gtk-3")]
unsafe extern "C" {
    fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> c_int;
    fn gtk_events_pending() -> c_int;
    fn gtk_main_iteration_do(blocking: c_int) -> c_int;
}

thread_local! {
    static READY: Cell<Option<bool>> = const { Cell::new(None) };
}

fn gtk_ready() -> bool {
    READY.with(|ready| {
        if let Some(ready) = ready.get() {
            return ready;
        }
        // SAFETY: called once, on the GPUI main thread, before any other
        // GTK use. The backend list must outlive the call (static string).
        let ok = unsafe {
            gdk_set_allowed_backends(c"x11".as_ptr());
            gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) != 0
        };
        if ok {
            // GTK is initialized, so tao's `gtk::init` only records that.
            Box::leak(Box::new(tao::event_loop::EventLoop::<()>::new()));
        }
        ready.set(Some(ok));
        ok
    })
}

pub fn parent(raw: RawWindowHandle) -> Result<RawWindowHandle, Unavailable> {
    let window = match raw {
        RawWindowHandle::Xcb(handle) => handle.window.get() as c_ulong,
        RawWindowHandle::Xlib(handle) => handle.window,
        RawWindowHandle::Wayland(_) => {
            return Err(Unavailable(
                "Web pages can't be embedded on Wayland yet.".into(),
            ));
        }
        _ => return Err(no_window()),
    };
    if !gtk_ready() {
        return Err(Unavailable("The web engine couldn't start.".into()));
    }
    // X window ids are server-wide, so GTK's own connection can parent to it.
    Ok(RawWindowHandle::Xlib(XlibWindowHandle::new(window)))
}

pub fn pump() {
    if READY.with(Cell::get) != Some(true) {
        return;
    }
    // SAFETY: GTK was initialized on this thread by `gtk_ready`.
    unsafe {
        while gtk_events_pending() != 0 {
            gtk_main_iteration_do(0);
        }
    }
}
