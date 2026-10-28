//! Pages on the operating system's own engine through wry: WebView2 on
//! Windows, WKWebView on macOS, WebKitGTK on Linux X11. Every request the
//! engine lets an app intercept goes through `buddy_core::browser`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use buddy_core::browser::{Blocked, Step, Url};
use futures::channel::mpsc::UnboundedSender;
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
    requests: Rc<Requests>,
}

impl WryHost {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            context: RefCell::default(),
            requests: Rc::default(),
        }
    }
}

/// Which page last asked for each address, newest last. WebKitGTK reports
/// downloads on the shared context, where only the first page's handler runs,
/// so a refused download is credited to the page whose navigation requested
/// it (#875: the notice landed on another tab). Other engines report each
/// download on its own page, which is also the one that requested it.
#[derive(Default)]
struct Requests(RefCell<VecDeque<(String, UnboundedSender<PageEvent>)>>);

impl Requests {
    const KEPT: usize = 32;

    fn record(&self, raw: &str, page: &UnboundedSender<PageEvent>) {
        let mut requests = self.0.borrow_mut();
        requests.retain(|(requested, _)| requested != raw);
        requests.push_back((raw.to_owned(), page.clone()));
        if requests.len() > Self::KEPT {
            requests.pop_front();
        }
    }

    /// The live page that last requested `raw`, if any.
    fn requester(&self, raw: &str) -> Option<UnboundedSender<PageEvent>> {
        self.0
            .borrow()
            .iter()
            .rev()
            .find(|(requested, page)| requested == raw && !page.is_closed())
            .map(|(_, page)| page.clone())
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
            checker,
            events,
            visible,
        } = config;
        let send = |events: &futures::channel::mpsc::UnboundedSender<PageEvent>, event| {
            // A closed channel means the tab is gone; nothing to report to.
            let _ = events.unbounded_send(event);
        };

        #[cfg(windows)]
        let frames = (checker.clone(), events.clone());
        let nav = events.clone();
        let requests = self.requests.clone();
        let requesters = self.requests.clone();
        let popups = events.clone();
        let downloads = events.clone();
        let permissions = events.clone();
        let titles = events.clone();
        let loads = events;
        let builder = WebViewBuilder::new_with_web_context(context)
            .with_url(url.as_str())
            .with_bounds(rect(bounds))
            .with_visible(visible)
            .with_focused(false)
            .with_clipboard(false)
            .with_devtools(false)
            .with_back_forward_navigation_gestures(false)
            .with_navigation_handler(move |raw| {
                let step = checker.navigation(&raw);
                match step {
                    Step::Allow => {
                        requests.record(&raw, &nav);
                        true
                    }
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
                let page = requesters.requester(&raw);
                send(
                    page.as_ref().unwrap_or(&downloads),
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
        // Without the frame guard a frame could reach the network unchecked, so
        // no page is shown at all.
        #[cfg(windows)]
        guard_frames(&webview, frames.0, frames.1).map_err(|err| {
            log::warn!("browser: frame guard failed: {err}");
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

    /// A WebKitGTK page created hidden and shown later stayed black in 6 of 7
    /// qualification runs on #875 (wry warns that such pages need several
    /// show/hide calls); created visible, 3 of 3 rendered.
    fn shows_at_creation(&self) -> bool {
        cfg!(target_os = "linux")
    }
}

/// WebView2 reports frame navigations through `FrameNavigationStarting`, which
/// wry doesn't hook (its navigation handler sees the main frame only), so frames
/// get the same decision here before any request leaves.
#[cfg(windows)]
fn guard_frames(
    webview: &WebView,
    checker: std::sync::Arc<buddy_core::browser::Checker>,
    events: futures::channel::mpsc::UnboundedSender<PageEvent>,
) -> windows::core::Result<()> {
    use webview2_com::{NavigationStartingEventHandler, take_pwstr};
    use windows::core::PWSTR;
    use wry::WebViewExtWindows as _;

    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut uri = PWSTR::null();
        // SAFETY: WebView2 writes a CoTaskMem string into `uri`, which
        // `take_pwstr` copies and frees.
        let raw = unsafe {
            args.Uri(&mut uri)?;
            take_pwstr(uri)
        };
        let step = checker.navigation(&raw);
        if step != Step::Allow {
            // SAFETY: a plain COM call on the live event arguments.
            unsafe { args.SetCancel(true)? };
        }
        if let Step::Block(reason) = step {
            let _ = events.unbounded_send(PageEvent::Blocked { raw, reason });
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: the WebView2 object keeps the handler for as long as it lives.
    unsafe {
        webview
            .webview()
            .add_FrameNavigationStarting(&handler, &mut token)
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

    fn url(&self) -> Option<String> {
        self.0.url().ok()
    }
}

#[cfg(test)]
mod tests {
    use futures::channel::mpsc;

    use super::{PageEvent, Requests};

    #[test]
    fn a_download_is_credited_to_the_live_page_that_last_requested_it() {
        let requests = Requests::default();
        let (first, mut first_events) = mpsc::unbounded::<PageEvent>();
        let (second, mut second_events) = mpsc::unbounded::<PageEvent>();
        requests.record("https://example.com/file", &first);
        requests.record("https://example.com/other", &first);
        requests.record("https://example.com/file", &second);
        let found = requests.requester("https://example.com/file").unwrap();
        found.unbounded_send(PageEvent::Title("x".into())).unwrap();
        assert!(second_events.try_recv().is_ok());
        assert!(first_events.try_recv().is_err());
        assert!(requests.requester("https://example.com/missing").is_none());
        // A closed tab's page is never credited.
        drop(second_events);
        assert!(requests.requester("https://example.com/file").is_none());
    }

    #[test]
    fn only_the_newest_requests_are_kept() {
        let requests = Requests::default();
        let (page, _events) = mpsc::unbounded::<PageEvent>();
        for index in 0..=Requests::KEPT {
            requests.record(&format!("https://example.com/{index}"), &page);
        }
        assert!(requests.requester("https://example.com/0").is_none());
        assert!(
            requests
                .requester(&format!("https://example.com/{}", Requests::KEPT))
                .is_some()
        );
    }
}
