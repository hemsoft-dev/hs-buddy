//! Pages on the operating system's own engine through wry: WebView2 on
//! Windows, WKWebView on macOS, WebKitGTK on Linux X11. Every request the
//! engine lets an app intercept goes through `buddy_core::browser`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use buddy_core::browser::{self, Blocked, Step, Url};
use gpui_kit::{Bounds, Pixels};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder,
};

use super::host::{Host, Page, PageConfig, PageEvent, Unavailable, no_window};

#[cfg(target_os = "linux")]
use super::linux as platform;

pub struct WryHost {
    profile: PathBuf,
    // One context per process: WebView2 requires every page to share one
    // environment, and WebKitGTK registers its handlers on it.
    context: RefCell<Option<WebContext>>,
}

impl WryHost {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            context: RefCell::default(),
        }
    }
}

struct Parent(RawWindowHandle);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the handle comes from the live GPUI window that owns the
        // page, and is used only for the synchronous build call.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

fn parent(raw: RawWindowHandle) -> Result<Parent, Unavailable> {
    #[cfg(target_os = "linux")]
    return platform::parent(raw).map(Parent);
    #[cfg(windows)]
    if wry::webview_version().is_err() {
        return Err(Unavailable(
            "Web pages need the Microsoft Edge WebView2 Runtime, which isn't installed.".into(),
        ));
    }
    #[cfg(not(target_os = "linux"))]
    Ok(Parent(raw))
}

fn rect(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        position: wry::dpi::LogicalPosition::new(
            f32::from(bounds.origin.x) as f64,
            f32::from(bounds.origin.y) as f64,
        )
        .into(),
        size: wry::dpi::LogicalSize::new(
            f32::from(bounds.size.width).max(1.0) as f64,
            f32::from(bounds.size.height).max(1.0) as f64,
        )
        .into(),
    }
}

impl Host for WryHost {
    fn create(
        &self,
        parent_handle: Option<RawWindowHandle>,
        config: PageConfig,
    ) -> Result<Rc<dyn Page>, Unavailable> {
        let parent = parent(parent_handle.ok_or_else(no_window)?)?;
        let mut context = self.context.borrow_mut();
        let context = context.get_or_insert_with(|| WebContext::new(Some(self.profile.clone())));

        let PageConfig {
            url,
            zoom,
            bounds,
            fixture,
            events,
        } = config;
        let send = |events: &futures::channel::mpsc::UnboundedSender<PageEvent>, event| {
            // A closed channel means the tab is gone; nothing to report to.
            let _ = events.unbounded_send(event);
        };

        let nav = events.clone();
        let popups = events.clone();
        let downloads = events.clone();
        let permissions = events.clone();
        let titles = events.clone();
        let loads = events;
        let builder = WebViewBuilder::new_with_web_context(context)
            .with_url(url.as_str())
            .with_bounds(rect(bounds))
            .with_visible(false)
            .with_focused(false)
            .with_clipboard(false)
            .with_devtools(false)
            .with_back_forward_navigation_gestures(false)
            .with_navigation_handler(move |raw| {
                let step = browser::navigation(&raw, fixture.as_ref());
                match step {
                    Step::Allow => true,
                    Step::Block(reason) => {
                        send(&nav, PageEvent::Blocked { raw, reason });
                        false
                    }
                    Step::External(url) => {
                        send(&nav, PageEvent::External(url));
                        false
                    }
                }
            })
            .with_new_window_req_handler(move |raw, _| {
                send(
                    &popups,
                    PageEvent::Blocked {
                        raw,
                        reason: Blocked::Popup,
                    },
                );
                NewWindowResponse::Deny
            })
            .with_download_started_handler(move |raw, _| {
                send(
                    &downloads,
                    PageEvent::Blocked {
                        raw,
                        reason: Blocked::Download,
                    },
                );
                false
            })
            .with_permission_handler(move |kind| {
                let _ = permissions.unbounded_send(PageEvent::Blocked {
                    raw: String::new(),
                    reason: Blocked::Permission(kind.to_string()),
                });
                PermissionResponse::Deny
            })
            .with_document_title_changed_handler(move |title| {
                send(&titles, PageEvent::Title(title))
            })
            .with_on_page_load_handler(move |event, url| {
                send(
                    &loads,
                    match event {
                        PageLoadEvent::Started => PageEvent::Loading(url),
                        PageLoadEvent::Finished => PageEvent::Loaded(url),
                    },
                )
            });
        #[cfg(windows)]
        let builder = {
            use wry::WebViewBuilderExtWindows as _;
            builder
                .with_browser_accelerator_keys(false)
                .with_default_context_menus(false)
        };
        #[cfg(target_os = "macos")]
        let builder = {
            use wry::WebViewBuilderExtDarwin as _;
            builder
                .with_data_store_identifier(buddy_core::browser::data_store_id(&self.profile))
                .with_allow_link_preview(false)
        };
        let webview = builder.build_as_child(&parent).map_err(|err| {
            log::warn!("browser: page creation failed: {err}");
            Unavailable("The web engine couldn't start.".into())
        })?;
        let page = WryPage(webview);
        page.zoom(zoom);
        Ok(Rc::new(page))
    }

    fn pump(&self) {
        #[cfg(target_os = "linux")]
        platform::pump();
    }

    fn needs_pump(&self) -> bool {
        cfg!(target_os = "linux")
    }
}

struct WryPage(WebView);

fn logged(action: &str, result: wry::Result<()>) {
    if let Err(err) = result {
        log::warn!("browser: {action} failed: {err}");
    }
}

impl Page for WryPage {
    fn load(&self, url: &Url) {
        logged("load", self.0.load_url(url.as_str()));
    }

    fn back(&self) {
        logged("back", self.0.evaluate_script("history.back()"));
    }

    fn forward(&self) {
        logged("forward", self.0.evaluate_script("history.forward()"));
    }

    fn reload(&self) {
        logged("reload", self.0.reload());
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        logged("resize", self.0.set_bounds(rect(bounds)));
    }

    fn set_visible(&self, visible: bool) {
        logged("visibility", self.0.set_visible(visible));
    }

    fn focus_parent(&self) {
        logged("focus", self.0.focus_parent());
    }

    fn zoom(&self, zoom: f64) {
        logged("zoom", self.0.zoom(zoom));
    }
}
