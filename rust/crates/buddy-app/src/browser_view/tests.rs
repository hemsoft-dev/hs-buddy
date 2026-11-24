use std::cell::RefCell;

use buddy_core::browser::Step;
use gpui_kit::{AppContext as _, Entity, VisualTestContext, px, size};

use super::*;

#[derive(Default)]
struct Log(RefCell<Vec<String>>);

impl Log {
    fn push(&self, entry: String) {
        self.0.borrow_mut().push(entry);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.borrow_mut())
    }

    fn has(&self, entry: &str) -> bool {
        self.0.borrow().iter().any(|logged| logged == entry)
    }
}

/// An engine that records what the view asks of it.
#[derive(Default)]
struct FakeHost {
    log: Rc<Log>,
    refuse: Option<Unavailable>,
    events: RefCell<Vec<mpsc::UnboundedSender<PageEvent>>>,
    checkers: RefCell<Vec<Arc<Checker>>>,
    /// Counts pumps when set; the engine then asks to be pumped, like GTK.
    pumps: Option<Cell<usize>>,
    /// What every page reports as its current address.
    url: Rc<RefCell<Option<String>>>,
    /// Asks for pages created on screen, like WebKitGTK.
    shows_at_creation: bool,
}

/// Records calls; `url` is the address the page reports, shared with its host.
struct FakePage(Rc<Log>, Rc<RefCell<Option<String>>>);

impl Page for FakePage {
    fn load(&self, url: &Url) {
        self.0.push(format!("load {url}"));
    }
    fn back(&self) {
        self.0.push("back".into());
    }
    fn forward(&self) {
        self.0.push("forward".into());
    }
    fn reload(&self) {
        self.0.push("reload".into());
    }
    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        self.0.push(format!(
            "bounds {}x{}",
            f32::from(bounds.size.width),
            f32::from(bounds.size.height)
        ));
    }
    fn set_visible(&self, visible: bool) {
        self.0.push(format!("visible {visible}"));
    }
    fn focus_parent(&self) {
        self.0.push("focus parent".into());
    }
    fn zoom(&self, zoom: f64) {
        self.0.push(format!("zoom {zoom}"));
    }
    fn url(&self) -> Option<String> {
        self.1.borrow().clone()
    }
}

impl Host for FakeHost {
    fn create(
        &self,
        _: Option<RawWindowHandleAlias>,
        config: PageConfig,
    ) -> Result<Rc<dyn Page>, Unavailable> {
        if let Some(reason) = &self.refuse {
            return Err(reason.clone());
        }
        self.log.push(format!("create {}", config.url));
        if config.visible {
            self.log.push("created visible".into());
        }
        self.events.borrow_mut().push(config.events);
        self.checkers.borrow_mut().push(config.checker);
        Ok(Rc::new(FakePage(self.log.clone(), self.url.clone())))
    }

    fn pump(&self) {
        if let Some(pumps) = &self.pumps {
            pumps.set(pumps.get() + 1);
        }
    }

    fn needs_pump(&self) -> bool {
        self.pumps.is_some()
    }

    fn shows_at_creation(&self) -> bool {
        self.shows_at_creation
    }
}

type RawWindowHandleAlias = raw_window_handle::RawWindowHandle;

impl FakeHost {
    fn send(&self, page: usize, event: PageEvent) {
        self.events.borrow()[page].unbounded_send(event).unwrap();
    }

    /// The engine's own decision when the first page navigates by itself.
    fn navigate(&self, raw: &str) -> Step {
        self.checkers.borrow()[0].navigation(raw)
    }

    fn pump_count(&self) -> usize {
        self.pumps.as_ref().map_or(0, Cell::get)
    }
}

// IP literals skip DNS, so these tests never touch the network.
const PAGE: &str = "https://93.184.215.14/";
const OTHER: &str = "https://93.184.215.15/next";

struct Harness {
    view: Entity<BrowserView>,
    visual: VisualTestContext,
    config: tempfile::TempDir,
}

impl Harness {
    fn config_file(&self) -> std::path::PathBuf {
        self.config.path().join("config.json")
    }

    fn saved(&self) -> Session {
        let text = std::fs::read_to_string(self.config_file()).unwrap();
        let config: buddy_core::config::AppConfig = serde_json::from_str(&text).unwrap();
        Session::load(&config, None)
    }

    fn update<R>(&mut self, f: impl FnOnce(&mut BrowserView, &mut Context<BrowserView>) -> R) -> R {
        let result = self.view.update(&mut self.visual, f);
        self.visual.run_until_parked();
        result
    }

    fn read<R>(&mut self, f: impl FnOnce(&BrowserView) -> R) -> R {
        self.view.read_with(&self.visual, |view, _| f(view))
    }
}

fn open_with(
    cx: &mut gpui_kit::TestAppContext,
    host: Rc<dyn Host>,
    config: buddy_core::config::AppConfig,
) -> Harness {
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    std::fs::write(&file, serde_json::to_string(&config).unwrap()).unwrap();
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(820.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| BrowserView::with_host(host, checker(), Some(file.clone()), window, cx))
            },
        )
        .unwrap()
    });
    let visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    Harness {
        view,
        visual,
        config: dir,
    }
}

fn open(cx: &mut gpui_kit::TestAppContext, host: &Rc<FakeHost>) -> Harness {
    open_with(cx, host.clone(), buddy_core::config::AppConfig::default())
}

fn shown(harness: &mut Harness) {
    harness.update(|view, cx| view.set_active(true, cx));
}

#[gpui_kit::test]
fn a_page_starts_after_its_check_and_shows_only_in_its_tab(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "Example", cx));

    let log = host.log.take();
    assert_eq!(
        log.first().map(String::as_str),
        Some(format!("create {PAGE}").as_str())
    );
    assert!(
        log.iter().any(|entry| entry.starts_with("bounds ")),
        "{log:?}"
    );
    assert_eq!(
        log.last().map(String::as_str),
        Some("visible true"),
        "{log:?}"
    );

    // The bookmark list hides the page.
    harness.update(|view, cx| view.select(None, cx));
    assert_eq!(host.log.take(), ["focus parent", "visible false"]);
    harness.update(|view, cx| view.select(Some(0), cx));
    assert_eq!(host.log.take(), ["visible true"]);

    // Another section hides it too, and no second page is started.
    harness.update(|view, cx| view.set_active(false, cx));
    assert_eq!(host.log.take(), ["focus parent", "visible false"]);
    harness.update(|view, cx| view.set_active(true, cx));
    assert_eq!(host.log.take(), ["visible true"]);
}

#[gpui_kit::test]
fn an_engine_that_asks_gets_its_showing_page_created_visible(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost {
        shows_at_creation: true,
        ..FakeHost::default()
    });
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "Example", cx));
    let log = host.log.take();
    assert!(
        log.iter().any(|entry| entry == "created visible"),
        "{log:?}"
    );
    // Already on screen, so it isn't shown a second time.
    assert!(!log.iter().any(|entry| entry == "visible true"), "{log:?}");
    harness.update(|view, cx| view.select(None, cx));
    assert_eq!(host.log.take(), ["focus parent", "visible false"]);

    // A tab that isn't showing while its page starts gets it hidden.
    harness.visual.update(|_, cx| Overlays::set(true, cx));
    harness.update(|view, cx| view.open(OTHER, "Other", cx));
    let log = host.log.take();
    assert!(
        log.iter().any(|entry| entry == &format!("create {OTHER}")),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry == "created visible"),
        "{log:?}"
    );
    harness.visual.update(|_, cx| Overlays::set(false, cx));
    harness.visual.run_until_parked();
    assert_eq!(host.log.take(), ["visible true"]);
}

#[gpui_kit::test]
fn an_open_menu_or_dialog_hides_the_page(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    host.log.take();

    harness.visual.update(|_, cx| Overlays::set(true, cx));
    harness.visual.run_until_parked();
    assert_eq!(host.log.take(), ["focus parent", "visible false"]);
    // Nested overlays: the page returns only after the last one closes.
    harness.visual.update(|_, cx| Overlays::set(true, cx));
    harness.visual.update(|_, cx| Overlays::set(false, cx));
    harness.visual.run_until_parked();
    assert!(host.log.take().is_empty());
    harness.visual.update(|_, cx| Overlays::set(false, cx));
    harness.visual.run_until_parked();
    assert_eq!(host.log.take(), ["visible true"]);
}

#[gpui_kit::test]
fn a_page_is_not_started_until_the_section_is_shown(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    assert!(host.log.take().is_empty());
    shown(&mut harness);
    assert!(host.log.has(&format!("create {PAGE}")));
    assert!(host.log.has("visible true"));
}

/// Names for the DNS check; anything else fails to resolve.
/// Names for the DNS check; anything else fails to resolve.
struct FakeLookup(Vec<(&'static str, std::net::IpAddr)>);

impl browser::Lookup for FakeLookup {
    fn lookup(&self, host: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        let found = self
            .0
            .iter()
            .filter(|(name, _)| *name == host)
            .map(|(_, ip)| *ip)
            .collect::<Vec<_>>();
        if found.is_empty() {
            Err(std::io::ErrorKind::NotFound.into())
        } else {
            Ok(found)
        }
    }
}

fn checker() -> Arc<Checker> {
    Arc::new(Checker::new(
        Arc::new(FakeLookup(vec![
            ("public.example", "93.184.215.14".parse().unwrap()),
            ("rebind.example", "192.168.1.10".parse().unwrap()),
        ])),
        None,
    ))
}

#[gpui_kit::test]
fn a_link_to_a_name_that_resolves_inside_is_refused_before_it_loads(
    cx: &mut gpui_kit::TestAppContext,
) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    // The engine asks before sending anything; the answer is already final.
    assert_eq!(host.navigate("https://public.example/next"), Step::Allow);
    assert_eq!(
        host.navigate("https://rebind.example/admin"),
        Step::Block(Blocked::Internal)
    );
    assert_eq!(
        host.navigate("https://missing.example/"),
        Step::Block(Blocked::Unresolved)
    );
}

#[gpui_kit::test]
fn a_bookmark_whose_name_resolves_inside_never_loads(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open("https://rebind.example/", "", cx));
    harness.update(|view, cx| view.open("https://missing.example/", "", cx));
    assert!(host.log.take().is_empty(), "no page may start");
    let notices = harness.read(|view| {
        view.tabs
            .iter()
            .map(|tab| tab.notice.clone().map(|notice| notice.text))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        notices,
        [
            Some(Blocked::Internal.to_string()),
            Some(Blocked::Unresolved.to_string())
        ]
    );
    // Try again starts the same check, not a page.
    harness.update(|view, cx| view.reload(cx));
    assert!(host.log.take().is_empty());
}

#[gpui_kit::test]
fn the_bookmark_list_selection_survives_a_restart(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    harness.update(|view, cx| view.select(None, cx));
    let saved = harness.saved();
    assert_eq!(saved.active, None);

    let mut config = buddy_core::config::AppConfig::default();
    saved.store(&mut config);
    let restored_host = Rc::new(FakeHost::default());
    let mut restored = open_with(cx, restored_host.clone(), config);
    shown(&mut restored);
    assert_eq!(restored.read(|view| view.active), None);
    assert!(restored_host.log.take().is_empty(), "no page may start");
}

/// The app's root, which holds the keyboard focus until something takes it.
struct Root {
    global_actions: usize,
    focus: gpui_kit::FocusHandle,
    browser: Entity<BrowserView>,
}

impl gpui_kit::Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement as _, ParentElement as _, Styled as _};
        gpui_kit::div()
            .key_context("BuddyApp")
            .on_action(
                cx.listener(|this, _: &crate::app::NextGlobalTab, _, _| this.global_actions += 1),
            )
            .on_action(
                cx.listener(|this, _: &crate::app::PreviousGlobalTab, _, _| {
                    this.global_actions += 1
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::app::CloseGlobalTab, _, _| this.global_actions += 1),
            )
            .track_focus(&self.focus)
            .size_full()
            .child(self.browser.clone())
    }
}

#[gpui_kit::test]
fn browser_focus_does_not_shadow_global_tab_accelerators(cx: &mut gpui_kit::TestAppContext) {
    // Both root focus and hosted-browser controls must route tab keys to the shell.
    use gpui_kit::{Bounds, Modifiers, Point, WindowBounds, WindowOptions, point};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    let config = buddy_core::config::AppConfig::default();
    std::fs::write(&file, serde_json::to_string(&config).unwrap()).unwrap();
    let host: Rc<dyn Host> = Rc::new(FakeHost::default());
    let (window, root) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        crate::app::init_navigation(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(820.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let browser = cx.new(|cx| {
                    BrowserView::with_host(host, checker(), Some(file.clone()), window, cx)
                });
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Root {
                        focus,
                        browser,
                        global_actions: 0,
                    }
                })
            },
        )
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    let browser = root.read_with(&visual, |root, _| root.browser.clone());
    browser.update(&mut visual, |view, cx| {
        view.set_active(true, cx);
        view.open(PAGE, "", cx);
        view.open(OTHER, "", cx);
    });
    visual.run_until_parked();
    let active = |visual: &mut VisualTestContext| browser.read_with(visual, |view, _| view.active);
    assert_eq!(active(&mut visual), Some(1));
    visual.simulate_keystrokes("secondary-tab");
    assert_eq!(
        active(&mut visual),
        Some(1),
        "global cycling must not mutate browser-only selection"
    );
    assert_eq!(root.read_with(&visual, |root, _| root.global_actions), 1);
    let view = visual.debug_bounds("browser-view").unwrap();
    visual.simulate_click(view.origin + point(px(40.), px(16.)), Modifiers::default());
    assert_eq!(active(&mut visual), None);
    visual.simulate_keystrokes("secondary-tab secondary-shift-tab secondary-f4");
    assert_eq!(active(&mut visual), None);
    assert_eq!(browser.read_with(&visual, |view, _| view.tabs.len()), 2);
    assert_eq!(root.read_with(&visual, |root, _| root.global_actions), 4);
}

#[gpui_kit::test]
fn browser_commands_cycle_through_the_list_and_close_pages(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    harness.update(|view, cx| view.open(OTHER, "", cx));
    assert_eq!(harness.read(|view| view.active), Some(1));
    harness.update(|view, cx| view.cycle(true, cx));
    assert_eq!(harness.read(|view| view.active), None);
    harness.update(|view, cx| view.cycle(true, cx));
    assert_eq!(harness.read(|view| view.active), Some(0));
    harness.update(|view, cx| view.cycle(false, cx));
    assert_eq!(harness.read(|view| view.active), None);
    harness.update(|view, cx| view.cycle(false, cx));
    assert_eq!(harness.read(|view| view.active), Some(1));
    harness.update(|view, cx| view.close_active(cx));
    assert_eq!(harness.read(|view| view.tabs.len()), 1);
    // The list itself never closes.
    harness.update(|view, cx| view.select(None, cx));
    harness.update(|view, cx| view.close_active(cx));
    assert_eq!(harness.read(|view| view.tabs.len()), 1);
}

#[gpui_kit::test]
fn the_engine_pump_stops_when_the_last_page_closes(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost {
        pumps: Some(Cell::new(0)),
        ..FakeHost::default()
    });
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    harness.visual.executor().advance_clock(PUMP * 5);
    harness.visual.run_until_parked();
    assert!(host.pump_count() >= 3);
    harness.update(|view, cx| view.close(0, cx));
    harness.visual.executor().advance_clock(PUMP * 3);
    harness.visual.run_until_parked();
    let stopped = host.pump_count();
    harness.visual.executor().advance_clock(PUMP * 10);
    harness.visual.run_until_parked();
    assert_eq!(host.pump_count(), stopped);
    assert!(!harness.read(|view| view.pumping));
    // A new page starts it again.
    harness.update(|view, cx| view.open(OTHER, "", cx));
    harness.visual.executor().advance_clock(PUMP * 5);
    harness.visual.run_until_parked();
    assert!(host.pump_count() > stopped);
}

#[gpui_kit::test]
fn refused_requests_show_why_and_keep_the_page(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    host.log.take();

    for (event, text) in [
        (
            PageEvent::Blocked {
                raw: "https://ads.example/".into(),
                reason: Blocked::Popup,
            },
            Blocked::Popup.to_string(),
        ),
        (
            PageEvent::Blocked {
                raw: String::new(),
                reason: Blocked::Permission("camera".into()),
            },
            Blocked::Permission("camera".into()).to_string(),
        ),
        (
            PageEvent::Blocked {
                raw: "https://files.example/a.zip".into(),
                reason: Blocked::Download,
            },
            Blocked::Download.to_string(),
        ),
    ] {
        host.send(0, event);
        harness.visual.run_until_parked();
        let notice = harness.read(|view| view.tabs[0].notice.clone());
        assert_eq!(notice.map(|notice| notice.text), Some(text));
        harness.update(|view, cx| view.dismiss(cx));
    }
    // The engine refuses a private address by itself; the view explains it.
    assert_eq!(
        host.navigate("http://192.168.1.1/"),
        Step::Block(Blocked::Internal)
    );
    assert!(
        host.log
            .take()
            .iter()
            .all(|entry| !entry.starts_with("load"))
    );
}

#[gpui_kit::test]
fn a_refused_download_or_popup_offers_what_its_notice_asks_for(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    let offered = |harness: &mut Harness| {
        harness.read(|view| {
            view.tabs[0]
                .notice
                .as_ref()
                .and_then(|notice| notice.offer.clone())
                .map(|url| url.to_string())
        })
    };
    let refuse = |harness: &mut Harness, raw: &str, reason: Blocked| {
        host.send(
            0,
            PageEvent::Blocked {
                raw: raw.into(),
                reason,
            },
        );
        harness.visual.run_until_parked();
    };

    // "Open the page externally to download": the page, not the file.
    refuse(
        &mut harness,
        "https://files.example/a.zip",
        Blocked::Download,
    );
    assert_eq!(offered(&mut harness).as_deref(), Some(PAGE));
    // A pop-up offers its own address, when the policy admits it.
    refuse(&mut harness, "https://93.184.215.16/popup", Blocked::Popup);
    assert_eq!(
        offered(&mut harness).as_deref(),
        Some("https://93.184.215.16/popup")
    );
    refuse(&mut harness, "http://192.168.1.1/", Blocked::Popup);
    assert_eq!(offered(&mut harness), None);
    // Other refusals offer nothing.
    refuse(&mut harness, "", Blocked::Permission("camera".into()));
    assert_eq!(offered(&mut harness), None);
}

#[gpui_kit::test]
fn a_mailto_link_is_offered_not_opened(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    assert!(matches!(
        host.navigate("mailto:someone@example.com"),
        Step::External(_)
    ));
    host.send(
        0,
        PageEvent::External(Url::parse("mailto:someone@example.com").unwrap()),
    );
    harness.visual.run_until_parked();
    let notice = harness.read(|view| view.tabs[0].notice.clone()).unwrap();
    assert_eq!(notice.offer.unwrap().scheme(), "mailto");
}

#[gpui_kit::test]
fn a_disallowed_bookmark_opens_a_tab_that_explains(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open("file:///C:/Windows/win.ini", "Local file", cx));
    harness.update(|view, cx| view.open("http://localhost:3000/", "Dev server", cx));
    assert!(host.log.take().is_empty(), "no page may start");
    let notices = harness.read(|view| {
        view.tabs
            .iter()
            .map(|tab| tab.notice.clone().map(|notice| notice.text))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        notices,
        [
            Some(Blocked::Scheme("file".into()).to_string()),
            Some(Blocked::Internal.to_string())
        ]
    );
    // Neither is saved for the next start.
    assert!(harness.saved().tabs.is_empty());
}

#[gpui_kit::test]
fn without_an_engine_the_tab_offers_the_system_browser(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost {
        refuse: Some(Unavailable(
            "Web pages can't be embedded on Wayland yet.".into(),
        )),
        ..FakeHost::default()
    });
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    let notice = harness.read(|view| view.tabs[0].notice.clone()).unwrap();
    assert!(
        notice
            .text
            .starts_with("Web pages can't be embedded on Wayland yet.")
    );
    assert_eq!(notice.offer.unwrap().as_str(), PAGE);
}

#[gpui_kit::test]
fn the_real_engine_falls_back_in_a_window_without_a_handle(cx: &mut gpui_kit::TestAppContext) {
    let profile = tempfile::tempdir().unwrap();
    let host: Rc<dyn Host> = Rc::new(wry_host::WryHost::new(profile.path().to_path_buf()));
    let mut harness = open_with(cx, host, buddy_core::config::AppConfig::default());
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    let notice = harness.read(|view| view.tabs[0].notice.clone()).unwrap();
    assert!(
        notice.text.starts_with("This window can't host web pages."),
        "{notice:?}"
    );
    assert!(notice.offer.is_some());
}

#[gpui_kit::test]
fn tabs_and_titles_are_saved_and_restored(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    harness.update(|view, cx| view.open(OTHER, "Second", cx));
    host.send(0, PageEvent::Title("Example Domain".into()));
    host.send(0, PageEvent::Loaded(format!("{PAGE}moved")));
    harness.visual.run_until_parked();
    harness.update(|view, cx| view.zoom_by(ZOOM_DELTA, cx));

    let saved = harness.saved();
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.tabs[0].title, "Example Domain");
    assert_eq!(saved.tabs[0].url, format!("{PAGE}moved"));
    assert_eq!(saved.active, Some(1));
    assert_eq!(saved.zoom, 1.1);

    // A new start restores them without starting any page until shown.
    let mut config = buddy_core::config::AppConfig::default();
    saved.store(&mut config);
    let restored_host = Rc::new(FakeHost::default());
    let mut restored = open_with(cx, restored_host.clone(), config);
    assert_eq!(restored.read(|view| view.tabs.len()), 2);
    assert_eq!(restored.read(|view| view.active), Some(1));
    assert!(restored_host.log.take().is_empty());
    shown(&mut restored);
    assert!(restored_host.log.has(&format!("create {OTHER}")));
    assert!(restored_host.log.has("zoom 1.1") || restored.read(|view| view.zoom) == 1.1);
}

#[gpui_kit::test]
fn closing_tabs_moves_the_selection(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    for raw in [PAGE, OTHER, "https://93.184.215.16/"] {
        harness.update(|view, cx| view.open(raw, "", cx));
    }
    harness.update(|view, cx| view.select(Some(1), cx));
    host.log.take();
    harness.update(|view, cx| view.close(1, cx));
    assert_eq!(harness.read(|view| view.active), Some(1));
    assert!(host.log.has("visible false"));
    harness.update(|view, cx| view.close(0, cx));
    assert_eq!(harness.read(|view| view.active), Some(0));
    harness.update(|view, cx| view.close(0, cx));
    assert_eq!(harness.read(|view| view.active), None);
    assert!(harness.saved().tabs.is_empty());
}

#[gpui_kit::test]
fn toolbar_actions_reach_the_page(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    host.log.take();
    harness.update(|view, cx| {
        view.back();
        view.forward();
        view.reload(cx);
        view.zoom_by(-10.0, cx);
    });
    assert_eq!(
        host.log.take(),
        [
            "back",
            "forward",
            "reload",
            &format!("zoom {}", browser::ZOOM_RANGE.0)
        ]
    );
}

#[gpui_kit::test]
fn the_page_follows_its_area_when_the_window_resizes(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    let first = host
        .log
        .take()
        .into_iter()
        .rfind(|entry| entry.starts_with("bounds "))
        .unwrap();
    harness.visual.simulate_resize(size(px(900.), px(650.)));
    harness.visual.run_until_parked();
    let after = host
        .log
        .take()
        .into_iter()
        .rfind(|entry| entry.starts_with("bounds "))
        .unwrap();
    assert_ne!(first, after);
    assert!(after.starts_with("bounds 900x"), "{after}");
}

#[gpui_kit::test]
fn the_selection_is_saved_among_the_saved_tabs(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open("file:///C:/notes.txt", "Refused", cx));
    harness.update(|view, cx| view.open(PAGE, "Kept", cx));
    assert_eq!(harness.read(|view| view.active), Some(1));
    let saved = harness.saved();
    assert_eq!(saved.tabs.len(), 1);
    assert_eq!(saved.active, Some(0));
    // A selected refused tab is not saved; the list is selected instead.
    harness.update(|view, cx| view.select(Some(0), cx));
    assert_eq!(harness.saved().active, None);
}

#[gpui_kit::test]
fn no_more_tabs_open_than_a_restart_restores(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    for n in 0..browser::MAX_TABS {
        harness.update(|view, cx| view.open(&format!("{PAGE}{n}"), "", cx));
    }
    harness.update(|view, cx| view.open(OTHER, "One too many", cx));
    assert_eq!(harness.read(|view| view.tabs.len()), browser::MAX_TABS);
    assert!(harness.read(|view| view.full));
    assert!(harness.visual.debug_bounds("browser-notice").is_some());
    assert_eq!(harness.saved().tabs.len(), browser::MAX_TABS);
    // Closing one makes room again.
    harness.update(|view, cx| view.close(0, cx));
    assert!(!harness.read(|view| view.full));
    harness.update(|view, cx| view.open(OTHER, "Now it fits", cx));
    assert_eq!(harness.read(|view| view.tabs.len()), browser::MAX_TABS);
    assert_eq!(harness.saved().active, Some(browser::MAX_TABS - 1));
}

#[gpui_kit::test]
fn reload_while_a_tab_starts_starts_nothing_more(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    // Reload before the first check and page creation have finished.
    harness.update(|view, cx| {
        view.open(PAGE, "", cx);
        view.reload(cx);
        view.reload(cx);
    });
    let created = host
        .log
        .take()
        .into_iter()
        .filter(|entry| entry.starts_with("create "))
        .count();
    assert_eq!(created, 1);
    assert!(harness.read(|view| view.tabs[0].page.is_some()));
}

#[gpui_kit::test]
fn the_toolbar_opens_the_page_and_the_notice_opens_its_offer(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    host.send(
        0,
        PageEvent::External(Url::parse("mailto:someone@example.com").unwrap()),
    );
    harness.visual.run_until_parked();
    harness.update(|view, cx| view.open_externally(cx));
    assert_eq!(harness.visual.opened_url().as_deref(), Some(PAGE));
    harness.update(|view, cx| view.open_offer(cx));
    assert_eq!(
        harness.visual.opened_url().as_deref(),
        Some("mailto:someone@example.com")
    );
}

#[gpui_kit::test]
fn a_route_change_without_a_load_is_followed_and_saved(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    // The page moves with history.pushState: no load event, a new address.
    let route = format!("{PAGE}inbox#unread");
    *host.url.borrow_mut() = Some(route.clone());
    harness.visual.executor().advance_clock(FOLLOW * 2);
    harness.visual.run_until_parked();
    assert_eq!(harness.read(|view| view.tabs[0].url.clone()), route);
    assert_eq!(harness.saved().tabs[0].url, route);
    // A refused address is never adopted.
    *host.url.borrow_mut() = Some("file:///C:/x".into());
    harness.visual.executor().advance_clock(FOLLOW * 2);
    harness.visual.run_until_parked();
    assert_eq!(harness.read(|view| view.tabs[0].url.clone()), route);
}

#[gpui_kit::test]
fn a_load_that_never_finishes_ends_with_a_notice(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    assert!(harness.read(|view| view.tabs[0].loading));
    // The engine never reports the load as finished (a refused TLS handshake
    // on WKWebView, say).
    harness.visual.executor().advance_clock(LOAD_LIMIT);
    harness.visual.run_until_parked();
    let notice = harness.read(|view| view.tabs[0].notice.clone()).unwrap();
    assert_eq!(notice.text, STALLED);
    assert_eq!(notice.offer.unwrap().as_str(), PAGE);
    assert!(!harness.read(|view| view.tabs[0].loading));
    // A load that does finish later clears it.
    host.send(0, PageEvent::Loaded(PAGE.into()));
    harness.visual.run_until_parked();
    assert_eq!(harness.read(|view| view.tabs[0].notice.clone()), None);
}

#[gpui_kit::test]
fn a_load_that_finishes_in_time_leaves_no_notice(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut harness = open(cx, &host);
    shown(&mut harness);
    harness.update(|view, cx| view.open(PAGE, "", cx));
    host.send(0, PageEvent::Loaded(PAGE.into()));
    harness.visual.run_until_parked();
    harness.visual.executor().advance_clock(LOAD_LIMIT * 2);
    harness.visual.run_until_parked();
    assert_eq!(harness.read(|view| view.tabs[0].notice.clone()), None);
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn page_entry_clears_the_tooltip_trigger_hover_without_parent_motion(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::{
        InteractiveElement as _, IntoElement, Render, StatefulInteractiveElement as _, Styled as _,
        Window, div, point,
    };
    struct Trigger(Rc<RefCell<Vec<bool>>>);
    impl Render for Trigger {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let transitions = self.0.clone();
            div()
                .id("tab-tooltip-trigger")
                .size(px(40.))
                .on_hover(move |hovered, _, _| transitions.borrow_mut().push(*hovered))
        }
    }
    let transitions = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_window(|_, _| Trigger(transitions.clone()));
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(point(px(10.), px(10.)), cx);
        window.draw(cx).clear(cx);
        assert_eq!(*transitions.borrow(), [true]);
        // GTK supplies the move to the page, so GPUI itself saw no move.
        linux::page_pointer(window, point(px(100.), px(100.)), cx);
        window.draw(cx).clear(cx);
        assert_eq!(*transitions.borrow(), [true, false]);
        // A redraw must not revive the old tooltip trigger.
        window.draw(cx).clear(cx);
        assert_eq!(*transitions.borrow(), [true, false]);
        window.simulate_mouse_move(point(px(10.), px(10.)), cx);
        assert_eq!(*transitions.borrow(), [true, false, true]);
    })
    .unwrap();
}

#[gpui_kit::test]
fn global_destination_restores_its_page_and_close_disposes_it(cx: &mut gpui_kit::TestAppContext) {
    let host = Rc::new(FakeHost::default());
    let mut h = open(cx, &host);
    shown(&mut h);
    h.update(|view, cx| view.open(PAGE, "First", cx));
    let first = h
        .view
        .read_with(&h.visual, |view, cx| view.destination(h.view.clone(), cx));
    h.update(|view, cx| view.open(OTHER, "Second", cx));
    h.visual.update(|window, cx| (first.restore)(window, cx));
    assert_eq!(h.read(|view| view.active_tab().unwrap().url.clone()), PAGE);
    h.update(|view, cx| view.open(PAGE, "Renamed", cx));
    assert_eq!(h.read(|view| view.tabs.len()), 2);
    h.visual.update(|_, cx| (first.close)(cx));
    assert_eq!(h.read(|view| view.tabs.len()), 1);
    assert_eq!(h.saved().tabs.len(), 1);
    assert!(host.log.has("visible false"));
}
