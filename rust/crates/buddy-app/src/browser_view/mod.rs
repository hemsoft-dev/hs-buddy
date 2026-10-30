//! Bookmarks' embedded browser: tabs of web pages beside the bookmark list.
//! What a page may do is decided in `buddy_core::browser`; the page itself
//! comes from a [`Host`], normally the operating system's engine.
//!
//! A native page is drawn above the GPUI window, so it is shown only while
//! its tab is active, the Bookmarks section is open and no GPUI overlay
//! (menu, dialog) would sit under it.

mod host;
pub(crate) mod keys;
#[cfg(target_os = "linux")]
mod linux;
mod render;
#[cfg(test)]
mod tests;
mod wry_host;

use std::cell::Cell;
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use buddy_core::browser::{
    self, Audit, Blocked, Checker, FixtureOrigin, Session, SystemLookup, Target, Url,
};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{
    AnyView, AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle, Global, Pixels, Window,
    actions,
};
use raw_window_handle::HasWindowHandle;

use crate::settings::Settings;
use host::{Host, Page, PageConfig, PageEvent, Unavailable};

/// Appends every refusal as one JSON line, for installed-app qualification.
/// Honored only with an isolated `BUDDY_CONFIG_PATH`.
pub const AUDIT_FILE_ENV: &str = "BUDDY_BROWSER_AUDIT_FILE";

const PUMP: Duration = Duration::from_millis(16);
/// How often live pages are asked for their address. wry reports loads, not
/// same-document route changes, so those are noticed this way.
const FOLLOW: Duration = Duration::from_secs(1);
/// How long a load may run before the tab says it didn't finish. Some engines
/// (WKWebView through wry) report no failed load at all.
pub(super) const LOAD_LIMIT: Duration = Duration::from_secs(30);
pub(super) const STALLED: &str =
    "The page didn't finish loading. Try again, or open it externally.";
pub(super) const ZOOM_DELTA: f64 = 0.1;
pub(super) const CONTEXT: &str = "BuddyBrowser";

actions!(browser, [NextTab, PreviousTab, CloseTab]);

/// Open GPUI overlays (menus, dialogs) that a native page would cover.
/// Whoever opens one counts it here while it is open.
#[derive(Default)]
pub struct Overlays(pub usize);

impl Global for Overlays {}

impl Overlays {
    pub fn set(open: bool, cx: &mut App) {
        let overlays = cx.default_global::<Overlays>();
        overlays.0 = if open {
            overlays.0 + 1
        } else {
            overlays.0.saturating_sub(1)
        };
    }

    fn any(cx: &App) -> bool {
        cx.try_global::<Overlays>()
            .is_some_and(|overlays| overlays.0 > 0)
    }
}

/// A reason shown above the tab's content, with an optional way out.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Notice {
    pub text: String,
    pub offer: Option<Url>,
}

pub(super) struct Tab {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub loading: bool,
    /// Distinguishes this tab's current load from earlier ones for the limit.
    load: u64,
    pub notice: Option<Notice>,
    events: mpsc::UnboundedSender<PageEvent>,
    page: Option<Rc<dyn Page>>,
    starting: bool,
    pub(super) bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    visible: bool,
}

impl Tab {
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

pub struct BrowserView {
    host: Rc<dyn Host>,
    checker: Arc<Checker>,
    pub(super) tabs: Vec<Tab>,
    /// `None` is the bookmark list.
    pub(super) active: Option<usize>,
    pub(super) home: Option<AnyView>,
    shown: bool,
    pub(crate) global_tabs: bool,
    pub(super) zoom: f64,
    fixture: Option<FixtureOrigin>,
    audit: Option<PathBuf>,
    window: AnyWindowHandle,
    /// Where the session is saved; `None` when the config path is unknown.
    config_file: Option<PathBuf>,
    pumping: bool,
    following: bool,
    /// The last open was refused because `MAX_TABS` tabs are open.
    pub(super) full: bool,
    next_id: u64,
    /// The tab shortcuts need the keyboard inside the browser. Buttons don't
    /// take focus on a click, so clicking a tab gives it to the browser.
    pub(super) focus: FocusHandle,
}

impl BrowserView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config_file = buddy_core::config::config_path().ok();
        let profile = config_file
            .as_deref()
            .map(browser::profile_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("buddy-browser-profile"));
        let host = Rc::new(wry_host::WryHost::new(profile));
        let lookup = Arc::new(SystemLookup {
            timeout: browser::LOOKUP_TIMEOUT,
        });
        let checker = Arc::new(Checker::new(lookup, FixtureOrigin::from_env()));
        Self::with_host(host, checker, config_file, window, cx)
    }

    fn with_host(
        host: Rc<dyn Host>,
        checker: Arc<Checker>,
        config_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fixture = checker.fixture().cloned();
        let isolated = std::env::var_os(buddy_core::config::CONFIG_PATH_ENV).is_some();
        let session = Session::load(&Settings::global(cx).config, fixture.as_ref());
        cx.observe_global::<Overlays>(|this, cx| this.sync(cx))
            .detach();
        let mut view = Self {
            host,
            checker,
            tabs: Vec::new(),
            active: None,
            home: None,
            shown: false,
            global_tabs: false,
            zoom: session.zoom,
            fixture,
            audit: isolated
                .then(|| std::env::var_os(AUDIT_FILE_ENV).map(PathBuf::from))
                .flatten(),
            window: Window::window_handle(window),
            config_file,
            pumping: false,
            following: false,
            full: false,
            next_id: 0,
            focus: cx.focus_handle(),
        };
        for saved in session.tabs {
            let tab = view.tab(saved.url, saved.title, cx);
            view.tabs.push(tab);
        }
        view.active = session.active;
        view
    }

    pub fn set_home(&mut self, home: AnyView, cx: &mut Context<Self>) {
        self.home = Some(home);
        cx.notify();
    }

    /// Called by the shell when the Bookmarks section opens or closes.
    pub fn set_active(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.shown = shown;
        self.sync(cx);
    }

    /// Opens `raw` in a new tab, or hands a `mailto:` link to the system.
    pub fn open(&mut self, raw: &str, title: &str, cx: &mut Context<Self>) {
        let target = browser::classify(raw, self.fixture.as_ref());
        if let Ok(Target::Web(url)) = &target
            && let Some(index) = self.tabs.iter().position(|tab| tab.url == url.as_str())
        {
            if !title.trim().is_empty() {
                self.tabs[index].title = title.to_owned();
            }
            self.select(Some(index), cx);
            return;
        }
        // Only `MAX_TABS` tabs are restored, so no more may be opened.
        self.full = !matches!(target, Ok(Target::External(_) | Target::Blank))
            && self.tabs.len() >= browser::MAX_TABS;
        if self.full {
            cx.notify();
            return;
        }
        match target {
            Ok(Target::External(url)) => cx.open_url(url.as_str()),
            Ok(Target::Blank) => {}
            Ok(Target::Web(url)) => {
                let tab = self.tab(url.to_string(), title.to_string(), cx);
                self.tabs.push(tab);
                self.select(Some(self.tabs.len() - 1), cx);
            }
            Err(reason) => {
                self.record(raw, &reason);
                let mut tab = self.tab(raw.to_string(), title.to_string(), cx);
                tab.notice = Some(Notice {
                    text: reason.to_string(),
                    offer: None,
                });
                self.tabs.push(tab);
                self.select(Some(self.tabs.len() - 1), cx);
            }
        }
        self.save(cx);
    }

    fn tab(&mut self, url: String, title: String, cx: &mut Context<Self>) -> Tab {
        self.next_id += 1;
        let id = self.next_id;
        let (events, mut receiver) = mpsc::unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                if this
                    .update(cx, |this, cx| this.page_event(id, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Tab {
            id,
            url,
            title,
            loading: false,
            load: 0,
            notice: None,
            events,
            page: None,
            starting: false,
            bounds: Rc::default(),
            visible: false,
        }
    }

    pub(super) fn select(&mut self, active: Option<usize>, cx: &mut Context<Self>) {
        self.active = active.filter(|index| *index < self.tabs.len());
        self.save(cx);
        self.sync(cx);
    }

    pub(super) fn close(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        self.full = false;
        if let Some(page) = &tab.page {
            page.set_visible(false);
        }
        self.active = match self.active {
            Some(active) if active == index => {
                (!self.tabs.is_empty()).then(|| index.min(self.tabs.len() - 1))
            }
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        self.save(cx);
        self.sync(cx);
    }

    pub(super) fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Shows exactly the page that should be on screen, and starts the
    /// active tab's page the first time it is needed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let showing = self.shown && !Overlays::any(cx);
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let visible = showing
                && self.active == Some(index)
                && tab.bounds.get().is_some()
                && tab.page.is_some();
            if tab.visible != visible {
                tab.visible = visible;
                if let Some(page) = &tab.page {
                    if !visible {
                        page.focus_parent();
                    }
                    page.set_visible(visible);
                }
            }
        }
        if self.shown
            && let Some(tab) = self.active_tab()
            && tab.page.is_none()
            && !tab.starting
            && tab.notice.is_none()
            // An engine that wants its page created on screen waits for the
            // tab's first layout (a frame): before it, a new tab has no area,
            // so its page was still created hidden, and stayed black (#875).
            && (tab.bounds.get().is_some() || !self.host.shows_at_creation())
            && let Ok(Target::Web(url)) = browser::classify(&tab.url, self.fixture.as_ref())
        {
            let id = tab.id;
            self.check(id, url, cx);
        }
        cx.notify();
    }

    /// Resolves `url` off the UI thread before the app loads it. The verdict
    /// is cached, so the engine's own check of the same load is instant.
    fn check(&mut self, id: u64, url: Url, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.starting = tab.page.is_none();
        tab.loading = true;
        self.watch_load(id, cx);
        let checker = self.checker.clone();
        let target = url.clone();
        let result = cx
            .background_executor()
            .spawn(async move { checker.check(&target) });
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |this, cx| this.checked(id, url, result, cx));
        })
        .detach();
    }

    fn checked(&mut self, id: u64, url: Url, result: Result<(), Blocked>, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        match result {
            Ok(()) => {
                tab.notice = None;
                tab.url = url.to_string();
                match tab.page.clone() {
                    Some(page) => page.load(&url),
                    None => self.start_page(id, url, cx),
                }
            }
            Err(reason) => {
                tab.starting = false;
                self.refuse(id, url.as_str(), reason, cx);
            }
        }
        cx.notify();
    }

    /// Engines pump the message loop while starting, so the page is built
    /// from a task, outside any GPUI update.
    fn start_page(&mut self, id: u64, url: Url, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let parent = window
                .update(cx, |_, window, _| {
                    <Window as HasWindowHandle>::window_handle(window)
                        .ok()
                        .map(|handle| handle.as_raw())
                })
                .ok()
                .flatten();
            let Ok(Some(config)) = this.update(cx, |this, cx| this.page_config(id, url, cx)) else {
                return;
            };
            let visible = config.visible;
            let result = host.create(parent, config);
            let _ = this.update(cx, |this, cx| this.page_started(id, result, visible, cx));
        })
        .detach();
    }

    fn page_config(&self, id: u64, url: Url, cx: &App) -> Option<PageConfig> {
        let index = self.index_of(id)?;
        let tab = &self.tabs[index];
        // The same test `sync` applies, before the page exists.
        let visible = self.host.shows_at_creation()
            && self.shown
            && !Overlays::any(cx)
            && self.active == Some(index)
            && tab.bounds.get().is_some();
        Some(PageConfig {
            url,
            zoom: self.zoom,
            bounds: tab.bounds.get().unwrap_or_default(),
            checker: self.checker.clone(),
            events: tab.events.clone(),
            visible,
            shortcuts: keys::claimed(cx),
        })
    }

    /// `visible` is whether the page was created on screen; `sync` then
    /// hides it if its tab stopped showing while the engine started.
    fn page_started(
        &mut self,
        id: u64,
        result: Result<Rc<dyn Page>, Unavailable>,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            // Closed while starting: the page is dropped here.
            return;
        };
        let tab = &mut self.tabs[index];
        tab.starting = false;
        match result {
            // A page that arrives after one is installed is dropped, never swapped in.
            Ok(_) if tab.page.is_some() => {}
            Ok(page) => {
                if let Some(bounds) = tab.bounds.get() {
                    page.set_bounds(bounds);
                }
                tab.visible = visible;
                tab.page = Some(page);
                self.start_pump(cx);
                self.start_following(cx);
            }
            Err(unavailable) => {
                tab.loading = false;
                tab.notice = Some(Notice {
                    text: format!("{unavailable} Open it externally instead."),
                    offer: Url::parse(&tab.url).ok(),
                });
            }
        }
        self.sync(cx);
    }

    fn start_pump(&mut self, cx: &mut Context<Self>) {
        if self.pumping || !self.host.needs_pump() {
            return;
        }
        self.pumping = true;
        #[cfg(target_os = "linux")]
        let window = self.window;
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            loop {
                host.pump();
                #[cfg(target_os = "linux")]
                let _ = window.update(cx, |_, window, cx| linux::dispatch_page_leave(window, cx));
                cx.background_executor().timer(PUMP).await;
                // Stop once no page is left; the next page restarts it.
                let pages = this.update(cx, |this, _| {
                    let any = this.tabs.iter().any(|tab| tab.page.is_some());
                    this.pumping = any;
                    any
                });
                if !matches!(pages, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Ends a load that hasn't finished within `LOAD_LIMIT` with a notice
    /// and an offer to open the page in the browser. A newer load, or a
    /// finished one, makes the timer moot.
    fn watch_load(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.load += 1;
        let load = tab.load;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOAD_LIMIT).await;
            let _ = this.update(cx, |this, cx| {
                let Some(index) = this.index_of(id) else {
                    return;
                };
                let tab = &mut this.tabs[index];
                if tab.load == load && tab.loading {
                    tab.loading = false;
                    tab.starting = false;
                    tab.notice = Some(Notice {
                        text: STALLED.into(),
                        offer: Url::parse(&tab.url).ok(),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads every live page's address each second and keeps the tab (and
    /// the saved session) on it. Stops when no page is left.
    fn start_following(&mut self, cx: &mut Context<Self>) {
        if self.following {
            return;
        }
        self.following = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FOLLOW).await;
                let pages = this.update(cx, |this, cx| {
                    this.follow_urls(cx);
                    let any = this.tabs.iter().any(|tab| tab.page.is_some());
                    this.following = any;
                    any
                });
                if !matches!(pages, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn follow_urls(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for tab in &mut self.tabs {
            let Some(current) = tab.page.as_ref().and_then(|page| page.url()) else {
                continue;
            };
            if current != tab.url
                && matches!(
                    browser::classify(&current, self.fixture.as_ref()),
                    Ok(Target::Web(_))
                )
            {
                tab.url = current;
                changed = true;
            }
        }
        if changed {
            self.save(cx);
            cx.notify();
        }
    }

    pub(super) fn page_event(&mut self, id: u64, event: PageEvent, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        match event {
            PageEvent::Loading(raw) => {
                self.watch_load(id, cx);
                let tab = &mut self.tabs[index];
                tab.loading = true;
                if matches!(
                    browser::classify(&raw, self.fixture.as_ref()),
                    Ok(Target::Web(_))
                ) {
                    tab.url = raw;
                }
            }
            PageEvent::Loaded(raw) => {
                let tab = &mut self.tabs[index];
                tab.loading = false;
                if tab
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.text == STALLED)
                {
                    tab.notice = None;
                }
                if matches!(
                    browser::classify(&raw, self.fixture.as_ref()),
                    Ok(Target::Web(_))
                ) {
                    tab.url = raw;
                }
                self.save(cx);
            }
            PageEvent::Title(title) => {
                self.tabs[index].title = title;
                self.save(cx);
            }
            PageEvent::Blocked { raw, reason } => self.refuse(id, &raw, reason, cx),
            PageEvent::External(url) => self.offer(id, url, cx),
        }
        cx.notify();
    }

    fn refuse(&mut self, id: u64, raw: &str, reason: Blocked, cx: &mut Context<Self>) {
        self.record(raw, &reason);
        if let Some(index) = self.index_of(id) {
            // The notice offers what its text asks for: the page for a
            // download, the pop-up's own address for a pop-up. Only a web
            // address the policy admits is offered.
            let offer = match reason {
                Blocked::Download => self.web(&self.tabs[index].url),
                Blocked::Popup => self.web(raw),
                _ => None,
            };
            let tab = &mut self.tabs[index];
            tab.loading = false;
            tab.notice = Some(Notice {
                text: reason.to_string(),
                offer,
            });
        }
        cx.notify();
    }

    fn web(&self, raw: &str) -> Option<Url> {
        match browser::classify(raw, self.fixture.as_ref()) {
            Ok(Target::Web(url)) => Some(url),
            _ => None,
        }
    }

    fn offer(&mut self, id: u64, url: Url, cx: &mut Context<Self>) {
        if let Some(index) = self.index_of(id) {
            self.tabs[index].notice = Some(Notice {
                text: "This link opens in another app.".into(),
                offer: Some(url),
            });
        }
        cx.notify();
    }

    fn record(&self, raw: &str, reason: &Blocked) {
        let audit = Audit::new(raw, reason);
        log::info!("browser: refused {audit:?}");
        let Some(path) = &self.audit else {
            return;
        };
        let line = serde_json::to_string(&audit).unwrap_or_default();
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{line}"));
        if let Err(err) = written {
            log::warn!("browser: audit write failed: {err}");
        }
    }

    pub(super) fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.tabs[index].notice = None;
        }
        self.sync(cx);
    }

    pub(super) fn back(&mut self) {
        if let Some(page) = self.active_tab().and_then(|tab| tab.page.as_ref()) {
            page.back();
        }
    }

    pub(super) fn forward(&mut self) {
        if let Some(page) = self.active_tab().and_then(|tab| tab.page.as_ref()) {
            page.forward();
        }
    }

    /// Reload, or retry a tab that never started.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let tab = &mut self.tabs[index];
        // A first load still being checked or started has nothing to retry yet.
        if tab.starting {
            return;
        }
        tab.notice = None;
        match &tab.page {
            Some(page) => page.reload(),
            None => {
                if let Ok(Target::Web(url)) = browser::classify(&tab.url, self.fixture.as_ref()) {
                    let id = tab.id;
                    self.check(id, url, cx);
                }
            }
        }
        cx.notify();
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab: the next or previous tab, the bookmark
    /// list included, wrapping around.
    pub(super) fn cycle(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.tabs.len() + 1;
        let position = self.active.map_or(0, |index| index + 1);
        let next = if forward {
            (position + 1) % count
        } else {
            (position + count - 1) % count
        };
        self.select(next.checked_sub(1), cx);
    }

    /// Ctrl+F4: close the selected page tab.
    pub(super) fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.close(index, cx);
        }
    }

    pub(super) fn zoom_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.zoom = browser::clamp_zoom(((self.zoom + delta) * 10.0).round() / 10.0);
        for page in self.tabs.iter().filter_map(|tab| tab.page.as_ref()) {
            page.zoom(self.zoom);
        }
        self.save(cx);
        cx.notify();
    }

    /// The toolbar's Open externally: always the page the tab shows.
    pub(super) fn open_externally(&mut self, cx: &mut Context<Self>) {
        let url = self.active_tab().and_then(|tab| self.web(&tab.url));
        if let Some(url) = url {
            cx.open_url(url.as_str());
        }
    }

    /// The notice's button: the link it offers (a `mailto:` link, the page
    /// itself when this window can't show it or a download was refused, or a
    /// refused pop-up's address).
    pub(super) fn open_offer(&mut self, cx: &mut Context<Self>) {
        let url = self
            .active_tab()
            .and_then(|tab| tab.notice.as_ref())
            .and_then(|notice| notice.offer.clone());
        if let Some(url) = url {
            cx.open_url(url.as_str());
        }
    }

    /// Keeps the page over the area its tab's content occupies. Returns
    /// whether this is the first known area, so the page may now be shown.
    pub(super) fn place(&self, id: u64, bounds: Bounds<Pixels>) -> bool {
        let Some(tab) = self.index_of(id).map(|index| &self.tabs[index]) else {
            return false;
        };
        let first = tab.bounds.get().is_none();
        if tab.bounds.get() != Some(bounds) {
            tab.bounds.set(Some(bounds));
            if let Some(page) = &tab.page {
                page.set_bounds(bounds);
            }
        }
        first
    }

    fn save(&self, cx: &mut Context<Self>) {
        // Refused tabs aren't saved, so the selection is re-counted among the
        // saved ones; a selected refused tab saves as the bookmark list.
        let kept = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| {
                matches!(
                    browser::classify(&tab.url, self.fixture.as_ref()),
                    Ok(Target::Web(_))
                )
            })
            .collect::<Vec<_>>();
        let session = Session {
            active: self
                .active
                .and_then(|active| kept.iter().position(|(index, _)| *index == active)),
            tabs: kept
                .into_iter()
                .map(|(_, tab)| browser::Tab {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                })
                .collect(),
            zoom: self.zoom,
        };
        let Some(path) = &self.config_file else {
            return;
        };
        if Session::load(&Settings::global(cx).config, self.fixture.as_ref()) != session {
            Settings::update_at(cx, path, |config| session.store(config));
        }
    }
}

impl BrowserView {
    pub(crate) fn destination(
        &self,
        entity: gpui_kit::Entity<Self>,
        cx: &gpui_kit::App,
    ) -> crate::shell::navigation::Destination {
        self.destination_at(self.active, entity, cx)
    }
    fn destination_at(
        &self,
        index: Option<usize>,
        entity: Entity<Self>,
        cx: &App,
    ) -> crate::shell::navigation::Destination {
        let id = index.and_then(|i| self.tabs.get(i)).map(|t| t.id);
        let _ = cx;
        let closing = entity.clone();
        crate::shell::navigation::Destination {
            key: format!("browser:{id:?}"),
            label: index
                .and_then(|i| self.tabs.get(i))
                .map(|t| format!("Browser: {}", t.label()))
                .unwrap_or_else(|| "Bookmarks".into()),
            section: Some(crate::app::Section::Bookmarks),
            badge: None,
            close: std::rc::Rc::new(move |cx| {
                closing.update(cx, |view, cx| {
                    if let Some(index) = id.and_then(|id| view.tabs.iter().position(|t| t.id == id))
                    {
                        view.close(index, cx);
                    }
                });
            }),
            restore: std::rc::Rc::new(move |window, cx| {
                let _ = window;
                entity.update(cx, |view, cx| {
                    let index = id.and_then(|id| view.tabs.iter().position(|t| t.id == id));
                    view.select(index, cx);
                });
            }),
        }
    }
}

impl BrowserView {
    pub(crate) fn is_home(&self) -> bool {
        self.active.is_none()
    }
}

impl BrowserView {
    pub(crate) fn destinations(
        &self,
        entity: Entity<Self>,
        cx: &App,
    ) -> Vec<crate::shell::navigation::Destination> {
        (0..self.tabs.len())
            .map(|i| self.destination_at(Some(i), entity.clone(), cx))
            .collect()
    }
}
