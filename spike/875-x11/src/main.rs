//! Reproduces hs-buddy#875's Linux failure outside GPUI: a plain X11 parent
//! window (like GPUI's), GTK initialized and pumped by hand exactly as
//! `browser_view/linux.rs` does, and pages switched the way `BrowserView::sync`
//! does (focus_parent + hide the old page, create the new one hidden, place it,
//! then show it). Each variant changes one step; captures go to the out dir.
//!
//! usage: spike-875-x11 <variant> <out-dir>   (SPIKE_TAG renames the captures)

use std::os::raw::{c_char, c_int, c_ulong};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilder};
use x11_dl::xlib;

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

struct Parent(c_ulong);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Xlib(XlibWindowHandle::new(self.0))) })
    }
}

fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        unsafe {
            while gtk_events_pending() != 0 {
                gtk_main_iteration_do(0);
            }
        }
        sleep(Duration::from_millis(16));
    }
}

fn capture(dir: &str, name: &str) {
    let path = format!("{dir}/{name}.png");
    let ok = Command::new("import")
        .args(["-window", "root", &path])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    println!("capture {name} ok={ok}");
}

fn area() -> Rect {
    Rect {
        position: LogicalPosition::new(0, 100).into(),
        size: LogicalSize::new(1000, 600).into(),
    }
}

fn page(context: &mut WebContext, parent: &Parent, text: &str, visible: bool) -> WebView {
    let html = format!(
        "<html><body style='background:#fff;color:#000;font:48px sans-serif'><h1>{text}</h1></body></html>"
    );
    WebViewBuilder::new_with_web_context(context)
        .with_html(html)
        .with_bounds(area())
        .with_visible(visible)
        .with_focused(false)
        .build_as_child(parent)
        .expect("page")
}

fn hide(page: &WebView) {
    page.focus_parent().unwrap();
    page.set_visible(false).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let variant = args[1].as_str();
    let dir = args[2].as_str();
    let tag = std::env::var("SPIKE_TAG").unwrap_or_else(|_| variant.to_owned());

    let x = xlib::Xlib::open().expect("xlib");
    let display = unsafe { (x.XOpenDisplay)(std::ptr::null()) };
    assert!(!display.is_null(), "no X display");
    let window = unsafe {
        let screen = (x.XDefaultScreen)(display);
        let root = (x.XRootWindow)(display, screen);
        let window = (x.XCreateSimpleWindow)(display, root, 0, 0, 1000, 700, 0, 0, 0x33_66_99);
        (x.XMapWindow)(display, window);
        (x.XFlush)(display);
        window
    };
    unsafe {
        gdk_set_allowed_backends(c"x11".as_ptr());
        assert!(gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) != 0, "gtk init");
    }
    Box::leak(Box::new(tao::event_loop::EventLoop::<()>::new()));

    let parent = Parent(window);
    let mut context = WebContext::new(Some(std::env::temp_dir().join(format!("spike-{tag}"))));
    let start_visible = variant == "second-visible";
    pump(500);

    let a = page(&mut context, &parent, "PAGE A", start_visible);
    a.set_bounds(area()).unwrap();
    a.set_visible(true).unwrap();
    pump(4000);
    capture(dir, &format!("{tag}-1-a"));
    if variant == "first" {
        return;
    }

    if variant != "second-nohide" {
        hide(&a);
    }
    pump(1500);
    let b = page(&mut context, &parent, "PAGE B", start_visible);
    b.set_bounds(area()).unwrap();
    if variant == "second-delay" {
        pump(2000);
    }
    b.set_visible(true).unwrap();
    if variant == "second-nohide" {
        hide(&a);
    }
    if variant == "second-toggle" {
        pump(300);
        b.set_visible(false).unwrap();
        pump(300);
        b.set_visible(true).unwrap();
    }
    pump(4000);
    capture(dir, &format!("{tag}-2-b"));

    hide(&b);
    a.set_visible(true).unwrap();
    pump(3000);
    capture(dir, &format!("{tag}-3-a-again"));
}
