//! WebKitGTK inside GPUI's X11 window. wry wants an Xlib parent and an
//! initialized GTK pumped by the host app; GPUI hands out XCB and has no
//! GTK. The `gtk` crate itself isn't a dependency here: its bindings are
//! flagged unmaintained, so this module talks to the three GTK/GDK calls it
//! needs directly and lets tao (wry's maintained sibling) set gtk-rs's own
//! "initialized" flag, which webkit2gtk checks.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_uint, c_ulong};

use gpui_kit::{App, Keystroke, MouseMoveEvent, Pixels, PlatformInput, Point, Window, point, px};
use raw_window_handle::{HasWindowHandle, RawWindowHandle, XlibWindowHandle};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ConnectionExt as _, EventMask, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyButMask, KeyPressEvent,
};
use x11rb::rust_connection::RustConnection;

use super::host::{Unavailable, no_window};
use super::keys::{self, Pressed};

#[link(name = "gdk-3")]
unsafe extern "C" {
    fn gdk_set_allowed_backends(backends: *const c_char);
    fn gdk_keyval_to_lower(keyval: c_uint) -> c_uint;
    fn gdk_keyval_name(keyval: c_uint) -> *const c_char;
}

#[link(name = "gtk-3")]
unsafe extern "C" {
    fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> c_int;
    fn gtk_events_pending() -> c_int;
    fn gtk_main_iteration_do(blocking: c_int) -> c_int;
    fn gtk_widget_add_events(widget: *mut c_void, events: c_int);
    fn gtk_window_list_toplevels() -> *mut GList;
}

#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_list_free(list: *mut GList);
}

#[link(name = "gobject-2.0")]
unsafe extern "C" {
    fn g_object_get_data(object: *mut c_void, key: *const c_char) -> *mut c_void;
    fn g_object_set_data(object: *mut c_void, key: *const c_char, data: *mut c_void);
    fn g_signal_connect_data(
        instance: *mut c_void,
        signal: *const c_char,
        handler: *const c_void,
        data: *mut c_void,
        destroy: *const c_void,
        flags: c_int,
    ) -> c_ulong;
}

#[repr(C)]
struct GList {
    data: *mut c_void,
    next: *mut GList,
    prev: *mut GList,
}

/// The leading fields of GDK 3's `GdkEventKey`, in its C layout.
#[repr(C)]
struct GdkEventKey {
    kind: c_int,
    _window: *mut c_void,
    _send_event: i8,
    time: u32,
    state: c_uint,
    keyval: c_uint,
    _length: c_int,
    _string: *mut c_char,
    hardware_keycode: u16,
    group: u8,
}

const GDK_KEY_PRESS: c_int = 8;
const SHIFT: c_uint = 1;
const CONTROL: c_uint = 1 << 2;
const ALT: c_uint = 1 << 3;
const MOD4: c_uint = 1 << 6;
const SUPER: c_uint = 1 << 26;
/// Set on each page window once its key handlers are connected; the value is
/// the X window of the GPUI window its keys are handed back to.
const PARENT: &CStr = c"buddy-key-parent";

thread_local! {
    static READY: Cell<Option<bool>> = const { Cell::new(None) };
    static ENTERED: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    static CLAIMED: RefCell<Vec<Keystroke>> = const { RefCell::new(Vec::new()) };
    /// Key codes whose press went to Buddy, so their release follows it.
    static HANDED: RefCell<HashSet<u16>> = RefCell::new(HashSet::new());
    static X: OnceCell<Option<(RustConnection, u32)>> = const { OnceCell::new() };
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

/// X11 sends a key to the window under the pointer, so while the pointer
/// rests over a page Buddy's shortcuts reach the page's GTK window (#880).
/// Each page window hands those back to the GPUI window it sits in; every
/// other key stays with the page.
pub fn hand_back(parent: RawWindowHandle, shortcuts: Vec<Keystroke>) {
    let RawWindowHandle::Xlib(handle) = parent else {
        return;
    };
    CLAIMED.replace(shortcuts);
    // SAFETY: on the GPUI main thread with GTK initialized (`gtk_ready`). The
    // list is GTK's own and freed here; its windows aren't referenced past
    // this call, and the handler gets GTK's key event for these signals.
    unsafe {
        let list = gtk_window_list_toplevels();
        let mut node = list;
        while !node.is_null() {
            let window = (*node).data;
            if g_object_get_data(window, PARENT.as_ptr()).is_null() {
                g_object_set_data(
                    window,
                    PARENT.as_ptr(),
                    std::ptr::without_provenance_mut(handle.window as usize),
                );
                // GDK_ENTER_NOTIFY_MASK. The page receives crossings even
                // though GPUI gets no motion over its native child.
                gtk_widget_add_events(window, 1 << 12);
                g_signal_connect_data(
                    window,
                    c"enter-notify-event".as_ptr(),
                    on_enter as *const c_void,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    0,
                );
                for signal in [c"key-press-event", c"key-release-event"] {
                    g_signal_connect_data(
                        window,
                        signal.as_ptr(),
                        on_key as *const c_void,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        0,
                    );
                }
            }
            node = (*node).next;
        }
        g_list_free(list);
    }
}

/// Runs before GTK hands the key to the page. Returns true when the key went
/// to Buddy instead.
unsafe extern "C" fn on_key(
    window: *mut c_void,
    event: *const GdkEventKey,
    _: *mut c_void,
) -> c_int {
    // SAFETY: GTK passes its own key event and the window the handler is
    // connected to.
    let (event, parent) = unsafe { (&*event, g_object_get_data(window, PARENT.as_ptr())) };
    let press = event.kind == GDK_KEY_PRESS;
    let code = event.hardware_keycode;
    let buddy = if press {
        // SAFETY: plain lookups; a name GDK returns is a static string.
        let name = unsafe { gdk_keyval_name(gdk_keyval_to_lower(event.keyval)) };
        if name.is_null() {
            return 0;
        }
        let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
        let pressed = Pressed {
            control: event.state & CONTROL != 0,
            alt: event.state & ALT != 0,
            shift: event.state & SHIFT != 0,
            platform: event.state & (MOD4 | SUPER) != 0,
            name: &name,
        };
        CLAIMED.with_borrow(|claimed| keys::is_claimed(claimed, &pressed))
    } else {
        HANDED.with_borrow_mut(|handed| handed.remove(&code))
    };
    if !buddy || !forward(parent.addr() as u32, event, press) {
        return 0;
    }
    if press {
        HANDED.with_borrow_mut(|handed| handed.insert(code));
    }
    1
}

/// Sends the key to the GPUI window as the X server would have. GPUI reads a
/// key event's code and modifier state, so it handles it like its own input.
fn forward(parent: u32, event: &GdkEventKey, press: bool) -> bool {
    X.with(|x| {
        let Some((connection, root)) = x.get_or_init(|| {
            x11rb::connect(None).ok().map(|(connection, screen)| {
                let root = connection.setup().roots[screen].root;
                (connection, root)
            })
        }) else {
            return false;
        };
        // Core key state: modifiers and buttons, with the XKB group in bits 13-14.
        let state = (event.state & 0x1fff) as u16 | (u16::from(event.group & 3) << 13);
        let key = KeyPressEvent {
            response_type: if press {
                KEY_PRESS_EVENT
            } else {
                KEY_RELEASE_EVENT
            },
            detail: event.hardware_keycode as u8,
            sequence: 0,
            time: event.time,
            root: *root,
            event: parent,
            child: x11rb::NONE,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::from(state),
            same_screen: true,
        };
        // An empty mask delivers the event to the window's creator: GPUI.
        let sent = connection
            .send_event(false, parent, EventMask::NO_EVENT, key)
            .is_ok();
        sent && connection.flush().is_ok()
    })
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

/// Queue the leave for after GTK's pump, outside its callback and any GPUI
/// update. Do not consume the crossing: the page still needs its own input.
unsafe extern "C" fn on_enter(window: *mut c_void, _: *const c_void, _: *mut c_void) -> c_int {
    // SAFETY: the signal belongs to a live GTK window tagged by `hand_back`.
    let parent = unsafe { g_object_get_data(window, PARENT.as_ptr()) }.addr() as u32;
    ENTERED.with_borrow_mut(|entered| entered.insert(parent));
    0
}

pub fn dispatch_page_leave(window: &mut Window, cx: &mut App) {
    let Ok(raw) = <Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let parent = match raw.as_raw() {
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        RawWindowHandle::Xlib(handle) => handle.window as u32,
        _ => return,
    };
    if ENTERED.with_borrow_mut(|entered| entered.remove(&parent)) {
        // Query at dispatch time: the pointer may have returned to Buddy
        // since GTK queued the crossing. X coordinates are physical pixels.
        X.with(|x| {
            let Some((connection, _)) = x.get_or_init(|| {
                x11rb::connect(None).ok().map(|(connection, screen)| {
                    let root = connection.setup().roots[screen].root;
                    (connection, root)
                })
            }) else {
                return;
            };
            if let Ok(cookie) = connection.query_pointer(parent)
                && let Ok(pointer) = cookie.reply()
            {
                let scale = window.scale_factor();
                page_pointer(
                    window,
                    point(
                        px(f32::from(pointer.win_x) / scale),
                        px(f32::from(pointer.win_y) / scale),
                    ),
                    cx,
                );
            }
        });
    }
}

/// Use GPUI's ordinary motion lifecycle to cancel hover and pending tooltips.
pub(super) fn page_pointer(window: &mut Window, position: Point<Pixels>, cx: &mut App) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: window.modifiers(),
        }),
        cx,
    );
}
