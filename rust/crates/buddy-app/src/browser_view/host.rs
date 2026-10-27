//! The platform webview behind a browser tab. A trait, so view tests run
//! without an engine and a platform without one falls back to the system
//! browser instead of failing.

use std::rc::Rc;
use std::sync::Arc;

use buddy_core::browser::{Blocked, Checker, Url};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::{Bounds, Pixels};
use raw_window_handle::RawWindowHandle;

/// What a page reports back, in engine order.
#[derive(Debug, Clone, PartialEq)]
pub enum PageEvent {
    Loading(String),
    Loaded(String),
    Title(String),
    Blocked { raw: String, reason: Blocked },
    External(Url),
}

pub struct PageConfig {
    pub url: Url,
    pub zoom: f64,
    pub bounds: Bounds<Pixels>,
    pub checker: Arc<Checker>,
    pub events: UnboundedSender<PageEvent>,
}

/// One live engine view. Every call is best effort: an engine error is
/// logged by the implementation and never panics the app.
pub trait Page {
    fn load(&self, url: &Url);
    fn back(&self);
    fn forward(&self);
    fn reload(&self);
    fn set_bounds(&self, bounds: Bounds<Pixels>);
    fn set_visible(&self, visible: bool);
    fn focus_parent(&self);
    fn zoom(&self, zoom: f64);
    /// The address the page shows now, including same-document changes
    /// (`history.pushState`, fragments) that fire no load event.
    fn url(&self) -> Option<String>;
}

/// Why this window can't embed pages. `Display` is the text the tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable(pub String);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Host {
    /// Builds a page inside the window. Must not run inside a GPUI update:
    /// engines pump the message loop while they start.
    fn create(
        &self,
        parent: Option<RawWindowHandle>,
        config: PageConfig,
    ) -> Result<Rc<dyn Page>, Unavailable>;

    /// Engines that need the app to drive their event loop (GTK) get a
    /// call about every frame while any page exists.
    fn pump(&self) {}

    fn needs_pump(&self) -> bool {
        false
    }
}

pub fn no_window() -> Unavailable {
    Unavailable("This window can't host web pages.".into())
}
