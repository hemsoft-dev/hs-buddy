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
use wry::{NewWindowResponse, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder};
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

thread_local! {
    /// With SPIKE_REPAINT=1, redraws the parent every frame as GPUI does.
    static REPAINT: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}

fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        REPAINT.with(|repaint| {
            if let Some(repaint) = repaint.borrow().as_ref() {
                repaint();
            }
        });
        unsafe {
            while gtk_events_pending() != 0 {
                gtk_main_iteration_do(0);
            }
        }
        sleep(Duration::from_millis(16));
    }
}

/// A parent like GPUI's X11 window (gpui-pre-linux 0.3.8 x11/window.rs): the
/// screen's 32-bit TrueColor ("transparent") visual with its own colormap.
unsafe fn argb_window(x: &xlib::Xlib, display: *mut xlib::Display, screen: c_int, root: c_ulong) -> c_ulong {
    unsafe {
        let mut info: xlib::XVisualInfo = std::mem::zeroed();
        assert!((x.XMatchVisualInfo)(display, screen, 32, xlib::TrueColor, &mut info) != 0, "no 32-bit visual");
        let colormap = (x.XCreateColormap)(display, root, info.visual, xlib::AllocNone);
        let mut attributes: xlib::XSetWindowAttributes = std::mem::zeroed();
        attributes.colormap = colormap;
        attributes.border_pixel = 0;
        attributes.background_pixel = 0xff_33_66_99;
        println!("parent: 32-bit visual 0x{:x}", info.visualid);
        (x.XCreateWindow)(display, root, 0, 0, 1000, 700, 0, 32, xlib::InputOutput as u32, info.visual,
            xlib::CWColormap | xlib::CWBorderPixel | xlib::CWBackPixel, &mut attributes)
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

/// `text` is inline HTML text, or a URL. With SPIKE_HANDLERS=1 the page gets the
/// app's handlers (wry_host.rs): private hosts refused, popups/downloads/permissions denied.
fn page(context: &mut WebContext, parent: &Parent, text: &str, visible: bool) -> WebView {
    let builder = WebViewBuilder::new_with_web_context(context);
    let builder = if text.starts_with("http") {
        builder.with_url(text)
    } else {
        builder.with_html(format!(
            "<html><body style='background:#fff;color:#000;font:48px sans-serif'><h1>{text}</h1></body></html>"
        ))
    };
    let builder = builder
        .with_bounds(area())
        .with_visible(visible)
        .with_focused(false);
    let builder = if std::env::var("SPIKE_HANDLERS").as_deref() == Ok("1") {
        builder
            .with_clipboard(false)
            .with_devtools(false)
            .with_back_forward_navigation_gestures(false)
            .with_navigation_handler(|url| {
                let refused = url.contains("127.0.0.2") || url.contains("127.0.0.3");
                println!("navigation {url} refused={refused}");
                !refused
            })
            .with_new_window_req_handler(|url, _| {
                println!("popup {url} denied");
                NewWindowResponse::Deny
            })
            .with_download_started_handler(|url, _| {
                println!("download {url} denied");
                false
            })
            .with_permission_handler(|kind| {
                println!("permission {kind} denied");
                PermissionResponse::Deny
            })
            .with_document_title_changed_handler(|title| println!("title {title}"))
            .with_on_page_load_handler(|event, url| {
                let stage = match event {
                    wry::PageLoadEvent::Started => "started",
                    wry::PageLoadEvent::Finished => "finished",
                };
                println!("load {stage} {url}");
            })
    } else {
        builder
    };
    builder.build_as_child(parent).expect("page")
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
        let window = if std::env::var("SPIKE_PARENT").as_deref() == Ok("argb") {
            argb_window(&x, display, screen, root)
        } else {
            (x.XCreateSimpleWindow)(display, root, 0, 0, 1000, 700, 0, 0, 0x33_66_99)
        };
        (x.XMapWindow)(display, window);
        (x.XFlush)(display);
        window
    };
    if std::env::var("SPIKE_REPAINT").as_deref() == Ok("1") {
        let gc = unsafe { (x.XCreateGC)(display, window, 0, std::ptr::null_mut()) };
        let fill = x.XFillRectangle;
        let foreground = x.XSetForeground;
        let flush = x.XFlush;
        let shade = std::cell::Cell::new(0u64);
        REPAINT.with(|repaint| {
            *repaint.borrow_mut() = Some(Box::new(move || unsafe {
                shade.set((shade.get() + 1) % 64);
                foreground(display, gc, 0xff_33_66_00 | (0x60 + shade.get() as c_ulong));
                fill(display, window, gc, 0, 0, 1000, 700);
                flush(display);
            }));
        });
    }
    unsafe {
        gdk_set_allowed_backends(c"x11".as_ptr());
        assert!(gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) != 0, "gtk init");
    }
    Box::leak(Box::new(tao::event_loop::EventLoop::<()>::new()));

    let parent = Parent(window);
    let mut context = WebContext::new(Some(std::env::temp_dir().join(format!("spike-{tag}"))));
    let start_visible = variant == "second-visible";
    pump(500);

    let first = std::env::var("SPIKE_A").unwrap_or_else(|_| "PAGE A".to_owned());
    let second = std::env::var("SPIKE_B").unwrap_or_else(|_| "PAGE B".to_owned());
    let a = page(&mut context, &parent, &first, start_visible);
    a.set_bounds(area()).unwrap();
    a.set_visible(true).unwrap();
    pump(7000);
    capture(dir, &format!("{tag}-1-a"));
    if variant == "first" {
        return;
    }

    if variant != "second-nohide" {
        hide(&a);
    }
    pump(1500);
    let b = page(&mut context, &parent, &second, start_visible);
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
