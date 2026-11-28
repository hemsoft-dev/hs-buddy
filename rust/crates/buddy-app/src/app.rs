//! Root view: the application shell around the active content.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use buddy_core::config::DisplayRect;
use gpui_kit::component::resizable::ResizableState;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div,
};

use crate::automation_view::AutomationView;
use crate::bookmarks_view::{BookmarksView, OpenInTab};
use crate::browser_view::BrowserView;
use crate::copilot_view::CopilotView;
use crate::crew_view::CrewView;
use crate::dashboard::{DashboardEvent, DashboardView};
use crate::github_view::GitHubView;
use crate::platform_placement;
use crate::ralph_view::RalphView;
use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::settings_view::{SettingsEvent, SettingsView};
use crate::shell::{activity_bar, status_bar, tab_bar, title_bar};
use crate::tempo_view::TempoView;
use crate::terminal_view::TerminalView;
use crate::todoist_view::TodoistView;
use crate::window_persistence::{Placement, SAVE_DELAY, WindowPersistence};
use crate::zoom::{self, zpx};

gpui_kit::actions!(
    buddy,
    [
        Quit,
        About,
        Reload,
        ToggleFullScreen,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleAssistant,
        ToggleTerminal,
        NextGlobalTab,
        PreviousGlobalTab,
        CloseGlobalTab,
        NavigationBack,
        NavigationForward
    ]
);

pub fn init_navigation(cx: &mut App) {
    cx.bind_keys([
        crate::key_binding::binding("secondary-tab", NextGlobalTab, Some("BuddyApp"), cx),
        crate::key_binding::binding(
            "secondary-shift-tab",
            PreviousGlobalTab,
            Some("BuddyApp"),
            cx,
        ),
        crate::key_binding::binding("secondary-f4", CloseGlobalTab, Some("BuddyApp"), cx),
        gpui_kit::KeyBinding::new(
            "alt-left",
            NavigationBack,
            Some("BuddyApp && !BuddyTerminal && !BuddyTerminalPaste"),
        ),
        gpui_kit::KeyBinding::new(
            "alt-right",
            NavigationForward,
            Some("BuddyApp && !BuddyTerminal && !BuddyTerminalPaste"),
        ),
    ]);
}

/// Activity-bar sections, mirroring the ids in `ActivityBar.tsx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    GitHub,
    Terminal,
    Tasks,
    Insights,
    Automation,
    Ralph,
    Crew,
    Tempo,
    Bookmarks,
    Copilot,
    Settings,
}

/// A specific view another part of the shell can open: Electron's view ids
/// such as `pr-my-prs`, `settings-accounts`, `automation-runs` or
/// `copilot-usage`, passed to its `onNavigate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewTarget {
    /// The section at its retained page.
    Section(Section),
    MyPrs,
    AutomationJobs,
    AutomationSchedules,
    AutomationRuns,
    SettingsAccounts,
    CopilotUsage,
}

impl From<Section> for ViewTarget {
    fn from(section: Section) -> Self {
        Self::Section(section)
    }
}

impl Section {
    pub const ALL: [Section; 11] = [
        Section::GitHub,
        Section::Terminal,
        Section::Tasks,
        Section::Insights,
        Section::Automation,
        Section::Ralph,
        Section::Crew,
        Section::Tempo,
        Section::Bookmarks,
        Section::Copilot,
        Section::Settings,
    ];

    /// The section with this `id`, if it is still one of ours.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.id() == id)
    }

    pub fn id(self) -> &'static str {
        match self {
            Section::GitHub => "github",
            Section::Terminal => "terminal",
            Section::Tasks => "tasks",
            Section::Insights => "insights",
            Section::Automation => "automation",
            Section::Ralph => "ralph",
            Section::Crew => "crew",
            Section::Tempo => "tempo",
            Section::Bookmarks => "bookmarks",
            Section::Copilot => "copilot",
            Section::Settings => "settings",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::GitHub => "GitHub",
            Section::Terminal => "Terminal",
            Section::Tasks => "Tasks",
            Section::Insights => "Insights",
            Section::Automation => "Automation",
            Section::Ralph => "Ralph Loops",
            Section::Crew => "The Crew",
            Section::Tempo => "Tempo",
            Section::Bookmarks => "Bookmarks",
            Section::Copilot => "Copilot",
            Section::Settings => "Settings",
        }
    }
}

pub struct BuddyApp {
    /// `None` means the dashboard is active.
    pub active_section: Option<Section>,
    /// The account the `gh` CLI is currently using, once resolved.
    pub active_account: Option<String>,
    pub(crate) dashboard: Entity<DashboardView>,
    settings_view: Entity<SettingsView>,
    pub(crate) github_view: Entity<GitHubView>,
    terminal_view: Entity<TerminalView>,
    terminal_panel: Entity<crate::terminal_view::panel::TerminalPanel>,
    copilot_view: Entity<CopilotView>,
    assistant: Entity<crate::copilot_view::chat::ChatView>,
    todoist_view: Entity<TodoistView>,
    tempo_view: Entity<TempoView>,
    pub(crate) automation_view: Entity<AutomationView>,
    ralph_view: Entity<RalphView>,
    crew_view: Entity<CrewView>,
    bookmarks_view: Entity<BookmarksView>,
    browser_view: Entity<BrowserView>,
    explorer_view: Entity<crate::explorer_view::ExplorerView>,
    folder_open: bool,
    window_state: WindowPersistence,
    focus: FocusHandle,
    pub(crate) tabs: crate::shell::navigation::Tabs,
    sidebar_split: SidebarSplit,
}

/// The sidebar split's state, owned here rather than retained under the
/// split's element id. The layout is a function of the saved sizes, the
/// row, the zoom and whether the assistant is open: when any of them
/// changes, the split starts from a new state laid out from them, so it
/// never depends on an earlier window width or the state GPUI Kit kept.
/// The retained state only matters during a drag; once the drag is saved
/// the split lays out again from the file. The assistant split inside the
/// workspace is owned the same way.
struct SidebarSplit {
    state: Entity<ResizableState>,
    /// What `state` was laid out from; `None` once a drag ended or while
    /// the split is off screen, so the next frame starts from the file.
    applied: Rc<Cell<Option<SidebarLayout>>>,
    /// The open assistant's width as last painted in window pixels, `None`
    /// while it is closed: a drag measures the workspace beside it.
    assistant: Rc<Cell<Option<gpui_kit::Pixels>>>,
    assistant_state: Entity<ResizableState>,
    /// What `assistant_state` was laid out from; `None` once a drag ended
    /// or while the assistant is closed.
    assistant_applied: Rc<Cell<Option<AssistantLayout>>>,
    /// Set when a drag's save could not read the file: the split lays out
    /// from the widths it showed until a later read (pane_sizes.rs, I4).
    awaiting_read: Rc<Cell<Option<Unsaved>>>,
}

/// A drag whose save could not read the file.
#[derive(Clone, Copy)]
struct Unsaved {
    /// The [`Settings`] read generation when the save failed.
    generation: u64,
    /// The sizes the save would have written (pane_sizes.rs, I2), which
    /// the split lays out from meanwhile, so a window resize keeps the
    /// drag; `None` when it would have written nothing.
    sizes: Option<[f64; 3]>,
}

/// The row the shell last laid out, which opening or closing the
/// assistant converts its width in (pane_sizes.rs, I3).
#[derive(Default)]
struct ShellRow(Option<buddy_core::pane_sizes::Row>);

impl gpui_kit::Global for ShellRow {}

/// Save the widths a drag of `divider` left on screen (pane_sizes.rs, I2)
/// into the file as it is now, then, I4: lay the split out again from the
/// file when the attempt read it (a save, or the resync after a failed
/// one), so the divider shows what a restart would. When it could not read
/// the file, lay it out from the widths it would have saved, computed from
/// the sizes in memory, until a later read.
fn save_drag(
    cx: &mut App,
    divider: buddy_core::pane_sizes::Divider,
    shown: buddy_core::pane_sizes::Shown,
    applied: impl FnOnce(),
    awaiting: &Cell<Option<Unsaved>>,
) {
    use buddy_core::pane_sizes;
    // A failed save keeps its error on screen.
    let generation = Settings::global(cx).generation();
    Settings::update(cx, move |config| {
        if let Some(sizes) = pane_sizes::after_drag(
            &config.ui.pane_sizes,
            divider,
            shown,
            config.ui.assistant_open,
        ) {
            config.ui.pane_sizes = sizes;
        }
    });
    if Settings::global(cx).generation() == generation {
        let ui = &Settings::global(cx).config.ui;
        let sizes = pane_sizes::after_drag(&ui.pane_sizes, divider, shown, ui.assistant_open)
            .map(|sizes| pane_sizes::entries(&sizes));
        awaiting.set(Some(Unsaved { generation, sizes }));
    } else {
        awaiting.set(None);
        applied();
    }
    cx.refresh_windows();
}

/// What the sidebar split lays out from: the saved sizes as the layout
/// reads them, whether the assistant is open, the zoom, and the row width
/// at 100%.
#[derive(Clone, Copy, PartialEq)]
struct SidebarLayout {
    sizes: [f64; 3],
    assistant_open: bool,
    unit: f32,
    available: f64,
}

/// What the assistant split lays out from: its limits (the saved width
/// and how far the row shrank them), the zoom, and the row width at 100%.
#[derive(Clone, Copy, PartialEq)]
struct AssistantLayout {
    limits: buddy_core::pane_sizes::AssistantLimits,
    unit: f32,
    available: f64,
}

impl SidebarSplit {
    fn new(cx: &mut App) -> Self {
        Self {
            state: cx.new(|_| ResizableState::default()),
            applied: Rc::default(),
            assistant: Rc::default(),
            assistant_state: cx.new(|_| ResizableState::default()),
            assistant_applied: Rc::default(),
            awaiting_read: Rc::default(),
        }
    }
}

impl BuddyApp {
    pub(crate) fn assistant_context(&self, cx: &App) -> crate::assistant_context::AssistantContext {
        if self.active_section == Some(Section::GitHub) {
            self.github_view.read(cx).assistant_context()
        } else {
            crate::assistant_context::AssistantContext::section(self.active_section)
        }
    }

    pub(crate) fn toggle_assistant(cx: &mut App) {
        let open = !Settings::global(cx).config.ui.assistant_open;
        // The content gives up or takes back the assistant's weight, so the
        // sidebar keeps its saved share (pane_sizes.rs, I3). A file Electron
        // already toggled the same way keeps its sizes.
        let row = cx.try_global::<ShellRow>().and_then(|row| row.0);
        Settings::update(cx, move |config| {
            if config.ui.assistant_open != open {
                config.ui.assistant_open = open;
                config.ui.pane_sizes = buddy_core::pane_sizes::after_assistant_toggle(
                    &config.ui.pane_sizes,
                    open,
                    row,
                );
            }
        });
        cx.refresh_windows();
    }
    /// Electron's `assistant:send-prompt`: another view's prompt, sent by
    /// the assistant with the sender's model. Electron's event reaches only
    /// an open assistant; native opens it first, so the prompt is never lost.
    pub(crate) fn send_assistant_prompt(
        &mut self,
        prompt: crate::copilot_view::chat::AssistantPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Settings::global(cx).config.ui.assistant_open {
            Self::toggle_assistant(cx);
        }
        // The assistant reads this shell for its context as it sends, so it
        // sends once this update has finished.
        let assistant = self.assistant.clone();
        window.defer(cx, move |window, cx| {
            assistant.update(cx, |view, cx| view.inject(prompt, window, cx));
        });
        cx.notify();
    }
    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = !Settings::global(cx).config.ui.terminal_open;
        if Settings::update(cx, |config| config.ui.terminal_open = open) {
            if open && !self.folder_open && self.active_section == Some(Section::GitHub) {
                let context = self.github_view.read(cx).terminal_context(cx);
                if let Some(context) = context {
                    self.terminal_panel
                        .update(cx, |panel, cx| panel.add(Some(context), cx));
                }
            }
            self.terminal_panel
                .update(cx, |panel, cx| panel.set_visible(open, cx));
            if !open {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    fn initial_focus(window: &mut Window, cx: &mut Context<Self>) -> FocusHandle {
        let focus = cx.focus_handle();
        // Root's Tab binding needs a focused descendant before the first click.
        // This handle is outside the tab order; Tab enters the first button.
        window.focus(&focus, cx);
        focus
    }

    /// Show a section (`None` is the dashboard) and remember it for the next
    /// launch; the dashboard pauses its refreshes while hidden.
    pub fn set_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
        if self.active_section != section
            && !Settings::update(cx, |config| {
                config.native.active_section = section.map(|section| section.id().to_string())
            })
        {
            cx.notify();
            return;
        }
        if section == Some(Section::Bookmarks) {
            self.browser_view
                .update(cx, |view, cx| view.select(None, cx));
        }
        let destination = self.destination(section, cx);
        self.tabs.open(destination);
        self.show_section(section, cx);
    }

    /// An activity-bar click. Electron's `openTab` counts the Dashboard and
    /// Bookmarks (`bookmarks-all`) opens; Insights has no sub-views here, so
    /// its entry stands for Electron's Insights view opens. Other sections
    /// count their own opens, or none, as their Electron views do.
    pub(crate) fn open_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
        self.set_section(section, cx);
        // Only an open that happened counts: a failed save keeps the old section.
        if self.active_section == section
            && matches!(section, None | Some(Section::Bookmarks | Section::Insights))
        {
            crate::view_activity::record(cx);
        }
    }

    /// Opens `target`'s section, then its view. The section's tab follows the
    /// view as it changes (see `observe_navigation`).
    pub(crate) fn open_target(&mut self, target: ViewTarget, cx: &mut Context<Self>) {
        match target {
            ViewTarget::Section(section) => self.set_section(Some(section), cx),
            ViewTarget::MyPrs => {
                self.set_section(Some(Section::GitHub), cx);
                self.github_view
                    .update(cx, |view, cx| view.show_status_prs(cx));
            }
            ViewTarget::AutomationJobs
            | ViewTarget::AutomationSchedules
            | ViewTarget::AutomationRuns => {
                self.set_section(Some(Section::Automation), cx);
                self.automation_view
                    .update(cx, |view, cx| view.show_target(target, cx));
            }
            ViewTarget::SettingsAccounts => {
                self.set_section(Some(Section::Settings), cx);
                self.settings_view
                    .update(cx, |view, cx| view.show_accounts(cx));
            }
            ViewTarget::CopilotUsage => {
                self.set_section(Some(Section::Copilot), cx);
                self.copilot_view.update(cx, |view, cx| view.show_usage(cx));
            }
        }
    }

    fn destination(
        &self,
        section: Option<Section>,
        cx: &App,
    ) -> crate::shell::navigation::Destination {
        match section {
            Some(Section::GitHub) => self
                .github_view
                .read(cx)
                .destination(self.github_view.clone(), cx),
            Some(Section::Settings) => self
                .settings_view
                .read(cx)
                .destination(self.settings_view.clone(), cx),
            Some(Section::Copilot) => self
                .copilot_view
                .read(cx)
                .destination(self.copilot_view.clone(), cx),
            Some(Section::Automation) => self
                .automation_view
                .read(cx)
                .destination(self.automation_view.clone(), cx),
            Some(Section::Tasks) => self
                .todoist_view
                .read(cx)
                .destination(self.todoist_view.clone(), cx),
            Some(Section::Ralph) => self
                .ralph_view
                .read(cx)
                .destination(self.ralph_view.clone(), cx),
            Some(Section::Crew) => self
                .crew_view
                .read(cx)
                .destination(self.crew_view.clone(), cx),
            Some(Section::Bookmarks) if self.browser_view.read(cx).is_home() => self
                .bookmarks_view
                .read(cx)
                .destination(self.bookmarks_view.clone()),
            Some(Section::Bookmarks) => self
                .browser_view
                .read(cx)
                .destination(self.browser_view.clone(), cx),
            _ => crate::shell::navigation::Destination::section(section),
        }
    }
    pub(crate) fn restore_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.github_view.update(cx, |view, cx| view.reconcile(cx));
        self.tabs.prune_unavailable(cx);
        for closed in std::mem::take(&mut self.tabs.closed) {
            (closed.close)(cx);
        }
        if let Some(destination) = self.tabs.current() {
            let section = destination.section;
            let saved = section.map(|s| s.id().to_owned());
            if Settings::global(cx).config.native.active_section != saved {
                Settings::update(cx, |config| config.native.active_section = saved.clone());
            }
            self.show_section(section, cx);
            if destination.key.starts_with("bookmarks:") {
                self.browser_view
                    .update(cx, |view, cx| view.select(None, cx));
            }
            (destination.restore)(window, cx);
            if destination.key.starts_with("folder:") {
                self.show_explorer(cx);
            }
        } else {
            if Settings::global(cx).config.native.active_section.is_some() {
                Settings::update(cx, |config| config.native.active_section = None);
            }
            self.show_section(None, cx);
            self.dashboard
                .update(cx, |view, cx| view.set_active(false, cx));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn show_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
        if section == Some(Section::Tasks) {
            self.todoist_view.update(cx, |view, cx| view.activate(cx));
        }
        if section == Some(Section::Tempo) {
            self.tempo_view.update(cx, |view, cx| view.activate(cx));
        }
        self.terminal_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Terminal), cx)
        });
        self.copilot_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Copilot), cx)
        });
        self.github_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::GitHub), cx)
        });
        self.ralph_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Ralph), cx)
        });
        self.crew_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Crew), cx)
        });
        self.bookmarks_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Bookmarks), cx)
        });
        self.browser_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Bookmarks), cx)
        });
        if section == Some(Section::Settings) && self.active_section != section {
            self.settings_view
                .update(cx, |settings, cx| settings.activate(cx));
        }
        if section != Some(Section::Settings) {
            self.settings_view
                .update(cx, |settings, _| settings.deactivate());
        }
        self.folder_open = false;
        self.active_section = section;
        self.dashboard.update(cx, |dashboard, cx| {
            dashboard.set_active(section.is_none(), cx)
        });
        cx.notify();
    }

    /// The window's placement, or `None` while it is minimized: Windows then
    /// reports the restore rectangle as windowed, which would forget that
    /// the window was maximized.
    fn window_placement(window: &Window, cx: &App) -> Option<Placement> {
        if !platform_placement::can_capture(window) {
            return None;
        }
        let reported = window.window_bounds();
        let outer = platform_placement::restore_bounds(window, cx)?;
        let bounds = match reported {
            gpui_kit::WindowBounds::Windowed(_) => gpui_kit::WindowBounds::Windowed(outer),
            gpui_kit::WindowBounds::Maximized(_) => gpui_kit::WindowBounds::Maximized(outer),
            gpui_kit::WindowBounds::Fullscreen(_) => gpui_kit::WindowBounds::Fullscreen(outer),
        };
        let display = window.display(cx);
        Some(Placement {
            bounds,
            native_geometry: platform_placement::native_geometry(window, cx),
            is_maximized: window.is_maximized(),
            is_full_screen: window.is_fullscreen(),
            native_display_uuid: display
                .as_ref()
                .and_then(|display| display.uuid().ok())
                .map(|id| id.to_string()),
            display: display
                .map(|display| platform_placement::display_area(display.as_ref(), cx).bounds),
        })
    }

    /// Save a move or resize once the window has stayed put for
    /// [`SAVE_DELAY`], so the placement survives the process being killed.
    fn window_bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(generation) = Self::window_placement(window, cx)
            .and_then(|placement| self.window_state.record(placement))
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, cx| {
                if this.window_state.save_if_current(generation) {
                    this.release_electron_display(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Release Electron's display selection after saving native placement.
    /// GPUI cannot validate Electron's ids, even when display bounds match:
    /// a replacement monitor can occupy those bounds with a different id.
    fn release_electron_display(&self, cx: &mut App) {
        let ui = &Settings::global(cx).config.ui;
        if ui.display_id == 0 {
            return;
        }
        Settings::update(cx, |config| {
            config.ui.display_id = 0;
            config.ui.display_bounds = DisplayRect::default();
            config.ui.display_work_area = DisplayRect::default();
        });
    }

    fn observe_settings(window: &Window, cx: &mut Context<Self>) {
        cx.observe_global_in::<Settings>(window, |this, window, cx| {
            this.github_view.update(cx, |view, cx| view.reconcile(cx));
            let pruned = this.tabs.prune_unavailable(cx);
            let saved = &Settings::global(cx).config.native.active_section;
            let section = saved.as_deref().and_then(Section::from_id);
            if this.active_section != section {
                this.tabs.open(this.destination(section, cx));
                this.show_section(section, cx);
            }
            if pruned {
                this.restore_tab(window, cx);
            }
            cx.notify();
        })
        .detach();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = Settings::global(cx).config.native.active_section.clone();
        let section = saved.as_deref().and_then(Section::from_id);
        let dashboard = cx.new(|cx| DashboardView::new(window, cx, section.is_none()));
        cx.observe(&dashboard, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&dashboard, |this, _, event, cx| {
            let DashboardEvent::Navigate(target) = event;
            this.open_target(*target, cx);
        })
        .detach();

        Self::observe_settings(window, cx);

        cx.observe_window_bounds(window, |this, window, cx| {
            this.window_bounds_changed(window, cx)
        })
        .detach();
        // Quitting (title-bar close, File → Exit, Ctrl+Q) writes the final
        // placement at once; quit handlers run while the window still exists.
        let handle = window.window_handle();
        // Closing a macOS window leaves the application running. Flush its
        // latest observed placement when the view is released as well.
        #[cfg(target_os = "macos")]
        cx.on_release(|this, cx| {
            if this.window_state.flush() && cx.has_global::<Settings>() {
                this.release_electron_display(cx);
            }
        })
        .detach();
        cx.on_app_quit(move |this, cx| {
            if let Ok(Some(placement)) =
                handle.update(cx, |_, window, cx| Self::window_placement(window, cx))
            {
                this.window_state.record(placement);
            }
            if this.window_state.flush() {
                this.release_electron_display(cx);
            }
            std::future::ready(())
        })
        .detach();

        // One-second tick for the clock and, later, the uptime badge.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                // jiff keeps the host time zone for five minutes. Look it up
                // again with each tick so a changed system zone shows within
                // about a second, as it did before.
                jiff::tz::reset_system_time_zone_cache();
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Resolve the active gh account off the UI thread, and re-check every
        // 30 seconds like the Electron status bar so `gh auth switch` shows up.
        cx.spawn(async move |this, cx| {
            loop {
                let rx =
                    cx.update(|cx| Runtime::global(cx).spawn(buddy_core::gh::active_account()));
                if let Ok(account) = rx.await
                    && this
                        .update(cx, |this, cx| {
                            if this.active_account != account {
                                this.copilot_view.update(cx, |view, cx| {
                                    view.set_default_account(account.clone(), cx)
                                });
                                this.crew_view.update(cx, |view, cx| {
                                    view.set_default_account(account.clone(), cx)
                                });
                                this.bookmarks_view.update(cx, |view, _| {
                                    view.set_default_account(account.clone())
                                });
                                this.assistant.update(cx, |view, cx| {
                                    view.set_default_account(account.clone(), cx)
                                });
                                this.active_account = account;
                                cx.notify();
                            }
                        })
                        .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
            }
        })
        .detach();

        let settings_view = cx.new(|cx| SettingsView::new(window, cx));
        let github_view = cx.new(|cx| GitHubView::new(window, cx));
        github_view.update(cx, |view, _| view.shell_navigation = true);
        cx.observe(&github_view, |_, _, cx| cx.notify()).detach();
        let terminal_view = cx.new(|cx| TerminalView::new(window, cx));
        let copilot_view = cx.new(|cx| CopilotView::new(window, cx));
        let assistant = cx.new(|cx| crate::copilot_view::chat::ChatView::new(window, cx));
        let assistant_source = cx.weak_entity();
        assistant.update(cx, |view, _| view.set_sidebar_source(assistant_source));
        let todoist_view = cx.new(|cx| TodoistView::new(window, cx));
        let tempo_view = cx.new(|cx| TempoView::new(window, cx));
        // The dispatcher claims queued runs whichever section is visible.
        let automation_view = cx.new(|cx| AutomationView::new(window, cx));
        automation_view.update(cx, |view, _| view.shell_navigation = true);
        cx.observe(&automation_view, |_, _, cx| cx.notify())
            .detach();
        automation_view.update(cx, |view, cx| view.start(cx));
        let ralph_view = cx.new(|cx| RalphView::new(window, cx));
        ralph_view.update(cx, |view, cx| view.start(cx));
        let crew_view = cx.new(|cx| CrewView::new(window, cx));
        crew_view.update(cx, |view, cx| view.start(cx));
        let bookmarks_view = cx.new(|cx| BookmarksView::new(window, cx));
        if section == Some(Section::Tempo) {
            tempo_view.update(cx, |view, cx| view.activate(cx));
        }
        if section == Some(Section::Tasks) {
            todoist_view.update(cx, |view, cx| view.activate(cx));
        }
        cx.subscribe_in(
            &github_view,
            window,
            |this, _, event: &crate::github_view::GitHubEvent, window, cx| match event {
                crate::github_view::GitHubEvent::PrepareAssistantReview(client, pull) => {
                    this.set_section(Some(Section::Copilot), cx);
                    this.copilot_view.update(cx, |view, cx| {
                        view.prepare_review(client.clone(), pull.clone(), cx)
                    });
                }
                crate::github_view::GitHubEvent::SendAssistantPrompt(prompt) => {
                    this.send_assistant_prompt(prompt.clone(), window, cx);
                }
                crate::github_view::GitHubEvent::StartRalph(handoff) => {
                    this.set_section(Some(Section::Ralph), cx);
                    this.ralph_view.update(cx, |view, cx| {
                        view.open_handoff(handoff.clone(), window, cx)
                    });
                }
            },
        )
        .detach();
        copilot_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Copilot), cx)
        });
        terminal_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Terminal), cx)
        });
        github_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::GitHub), cx)
        });
        ralph_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Ralph), cx)
        });
        crew_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Crew), cx)
        });
        bookmarks_view.update(cx, |view, cx| {
            view.set_active(section == Some(Section::Bookmarks), cx)
        });
        // Bookmarks open as tabs of the browser, whose first tab is the list.
        let browser_view = cx.new(|cx| BrowserView::new(window, cx));
        browser_view.update(cx, |view, _| view.global_tabs = true);
        browser_view.update(cx, |view, cx| {
            view.set_home(bookmarks_view.clone().into(), cx);
            view.set_active(section == Some(Section::Bookmarks), cx)
        });
        cx.subscribe(&bookmarks_view, |this, _, event: &OpenInTab, cx| {
            this.browser_view
                .update(cx, |view, cx| view.open(&event.url, &event.title, cx));
        })
        .detach();
        cx.subscribe(&settings_view, |this, _, event, cx| match event {
            SettingsEvent::ClearCache => {
                this.dashboard
                    .update(cx, |dashboard, cx| dashboard.clear_cached_data(cx));
                this.github_view.update(cx, |view, cx| view.clear_cache(cx));
            }
        })
        .detach();
        let terminal_panel =
            cx.new(|cx| crate::terminal_view::panel::TerminalPanel::new(window, cx));
        cx.subscribe(
            &terminal_panel,
            |this, _, event: &crate::explorer_view::OpenPath, cx| {
                this.show_folder(event.0.clone(), cx);
            },
        )
        .detach();
        let explorer_view = cx.new(|_| crate::explorer_view::ExplorerView::new());
        cx.subscribe(
            &terminal_view,
            |this, _, event: &crate::explorer_view::OpenPath, cx| {
                this.show_folder(event.0.clone(), cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &explorer_view,
            window,
            |this, _, _: &crate::explorer_view::Close, window, cx| {
                if let Some(key) = this.tabs.active.clone() {
                    this.tabs.close(&key);
                }
                this.restore_tab(window, cx);
            },
        )
        .detach();
        let mut this = Self {
            active_section: section,
            active_account: None,
            dashboard,
            settings_view,
            github_view,
            terminal_view,
            terminal_panel,
            copilot_view,
            assistant,
            todoist_view,
            tempo_view,
            automation_view,
            ralph_view,
            crew_view,
            bookmarks_view,
            browser_view,
            explorer_view,
            folder_open: false,
            window_state: WindowPersistence::new(),
            focus: Self::initial_focus(window, cx),
            tabs: crate::shell::navigation::Tabs::default(),
            sidebar_split: SidebarSplit::new(cx),
        };
        this.tabs
            .open(crate::shell::navigation::Destination::section(None));
        let restored = this
            .browser_view
            .read(cx)
            .destinations(this.browser_view.clone(), cx);
        if !restored.is_empty() {
            this.tabs.items.push(
                this.bookmarks_view
                    .read(cx)
                    .destination(this.bookmarks_view.clone()),
            );
        }
        this.tabs.items.extend(restored);
        if section.is_some() {
            this.tabs.open(this.destination(section, cx));
        }
        this.observe_navigation(cx);
        this.window_bounds_changed(window, cx);
        // Electron's tab list opens the Dashboard when its window starts.
        crate::view_activity::record(cx);
        this
    }

    fn show_explorer(&mut self, cx: &mut Context<Self>) {
        // Hide OS-hosted browser content and pause hidden dashboard/terminal work.
        self.browser_view
            .update(cx, |view, cx| view.set_active(false, cx));
        self.dashboard
            .update(cx, |view, cx| view.set_active(false, cx));
        self.terminal_view
            .update(cx, |view, cx| view.set_active(false, cx));
        self.folder_open = true;
        cx.notify();
    }

    /// A terminal's Open Folder: Electron's `openTab('folder-view:…')`.
    fn show_folder(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        crate::view_activity::record(cx);
        self.show_explorer(cx);
        self.explorer_view
            .update(cx, |view, cx| view.open(path, cx));
    }

    fn open_folder(
        &mut self,
        _: &crate::explorer_view::OpenFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_explorer(cx);
        self.explorer_view
            .update(cx, |view, cx| view.choose(window, cx));
    }

    fn observe_navigation(&self, cx: &mut Context<Self>) {
        cx.observe(&self.github_view, |this, entity, cx| {
            if this.active_section == Some(Section::GitHub)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.settings_view, |this, entity, cx| {
            if this.active_section == Some(Section::Settings)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.copilot_view, |this, entity, cx| {
            if this.active_section == Some(Section::Copilot)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.automation_view, |this, entity, cx| {
            if this.active_section == Some(Section::Automation)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.todoist_view, |this, entity, cx| {
            if this.active_section == Some(Section::Tasks)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.ralph_view, |this, entity, cx| {
            if this.active_section == Some(Section::Ralph)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.crew_view, |this, entity, cx| {
            if this.active_section == Some(Section::Crew)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let _ = entity;
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.browser_view, |this, entity, cx| {
            if this.active_section == Some(Section::Bookmarks)
                && !this.folder_open
                && this.tabs.active.is_some()
            {
                let live = entity.read(cx).destinations(entity.clone(), cx);
                let removed: Vec<_> = this
                    .tabs
                    .items
                    .iter()
                    .filter(|tab| {
                        tab.key.starts_with("browser:") && !live.iter().any(|d| d.key == tab.key)
                    })
                    .map(|d| d.key.clone())
                    .collect();
                for key in removed {
                    this.tabs.close(&key);
                }
                for destination in live {
                    if !this.tabs.items.iter().any(|d| d.key == destination.key) {
                        this.tabs.items.push(destination);
                    }
                }
                this.tabs.open(this.destination(this.active_section, cx));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.bookmarks_view, |this, entity, cx| {
            if this.active_section == Some(Section::Bookmarks)
                && this.browser_view.read(cx).is_home()
                && this.tabs.active.is_some()
            {
                this.tabs.open(entity.read(cx).destination(entity.clone()));
                cx.notify();
            }
        })
        .detach();
        cx.observe(&self.explorer_view, |this, entity, cx| {
            if this.folder_open {
                let destination = entity.read(cx).destination(entity.clone());
                if destination.key != "folder:chooser"
                    && this.tabs.active.as_deref() == Some("folder:chooser")
                {
                    this.tabs.close("folder:chooser");
                }
                this.tabs.open(destination);
                cx.notify();
            }
        })
        .detach();
    }

    /// The section's sidebar list, or `None` for a section without one.
    fn render_navigation(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        use gpui_kit::component::{
            Selectable as _,
            button::{Button, ButtonVariants as _},
        };
        if self.active_section == Some(Section::Copilot) {
            // Electron's Copilot sidebar: its own sections, not a link list.
            return (!self.folder_open)
                .then(|| self.copilot_view.update(cx, |view, cx| view.sidebar(cx)));
        }
        let links = match self.active_section {
            Some(Section::GitHub) => self
                .github_view
                .read(cx)
                .sidebar(self.github_view.clone(), cx),
            Some(Section::Automation) => self
                .automation_view
                .read(cx)
                .sidebar(self.automation_view.clone())
                .into_iter()
                .map(crate::shell::navigation::SidebarLink::from)
                .collect(),
            _ => Vec::new(),
        };
        if links.is_empty() || self.folder_open {
            return None;
        }
        // The divider beside it is the resizable split's handle.
        let list = v_flex()
            .id("shell-sidebar")
            .size_full()
            .p_2()
            .gap_1()
            .children(links.into_iter().enumerate().map(|(i, link)| {
                let d = &link.destination;
                let selected = self.tabs.active.as_ref() == Some(&d.key);
                let badge = d.badge.as_ref().map(|badge| {
                    if let Some(progress) = badge.progress {
                        use gpui_kit::component::Sizable as _;
                        let color = if progress <= 25.0 {
                            0x4ec9b0
                        } else if progress <= 50.0 {
                            0xdcd34a
                        } else if progress <= 75.0 {
                            0xe89b3c
                        } else {
                            0xe85d5d
                        };
                        gpui_kit::component::progress::ProgressCircle::new(("badge", i))
                            .with_size(zoom::scaled(cx, 40.0))
                            .text_size(zpx(10.0))
                            .value(progress)
                            .color(gpui_kit::rgb(color))
                            .accessibility_label(badge.tooltip.clone())
                            .child(badge.count.to_string())
                            .into_any_element()
                    } else {
                        div().child(badge.count.to_string()).into_any_element()
                    }
                });
                Button::new(("navigation-item", i))
                    .ghost()
                    .label(d.label.clone())
                    .accessibility_label(d.label.clone())
                    .selected(selected)
                    .tooltip(
                        d.badge
                            .as_ref()
                            .map(|b| b.tooltip.clone())
                            .unwrap_or_else(|| d.label.clone()),
                    )
                    .children(badge)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(destination) = link.open(cx) {
                            this.tabs.open(destination);
                            this.restore_tab(window, cx);
                        }
                    }))
            }));
        Some(list.into_any_element())
    }

    /// The sidebar beside the workspace, split as Electron's Allotment
    /// splits them: each pane takes its `ui.paneSizes` share of the row
    /// ([`buddy_core::pane_sizes::layout`]) and a drag of the divider saves
    /// the widths it shows.
    fn render_sidebar_split(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use buddy_core::pane_sizes::{self, Row};
        use gpui_kit::component::resizable::{h_resizable, resizable_panel};
        let navigation = self.render_navigation(cx);
        let unit = f32::from(zoom::scaled(cx, 1.0));
        // The row beside the activity bar, in pixels at 100%.
        let available = f64::from(
            f32::from(window.viewport_size().width) / unit - activity_bar::ACTIVITY_BAR_WIDTH,
        );
        let row = Row {
            available,
            sidebar: navigation.is_some(),
        };
        cx.set_global(ShellRow(Some(row)));
        // A drag whose save could not read the file kept its state; the
        // first read since lays both splits out from the file (I4).
        let generation = Settings::global(cx).generation();
        if self
            .sidebar_split
            .awaiting_read
            .get()
            .is_some_and(|waiting| waiting.generation != generation)
        {
            self.sidebar_split.awaiting_read.set(None);
            self.sidebar_split.applied.set(None);
            self.sidebar_split.assistant_applied.set(None);
        }
        let ui = &Settings::global(cx).config.ui;
        let assistant_open = ui.assistant_open;
        // Until then a drag that could not be saved lays out from the
        // widths it showed, which a window resize keeps.
        let sizes = self
            .sidebar_split
            .awaiting_read
            .get()
            .and_then(|waiting| waiting.sizes)
            .unwrap_or_else(|| pane_sizes::entries(&ui.pane_sizes));
        let layout = pane_sizes::layout(&sizes, assistant_open, row);
        let workspace = v_flex()
            .id("shell-workspace")
            .debug_selector(|| "shell-workspace".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .child(self.render_workspace(layout.assistant, row, cx));
        let (Some(navigation), Some(sidebar)) = (navigation, layout.sidebar) else {
            self.sidebar_split.applied.set(None);
            return h_flex().size_full().child(workspace).into_any_element();
        };
        let key = SidebarLayout {
            sizes,
            assistant_open,
            unit,
            available,
        };
        if self.sidebar_split.applied.get() != Some(key) {
            self.sidebar_split.state = cx.new(|_| ResizableState::default());
            self.sidebar_split.applied.set(Some(key));
        }
        let limits = sidebar.limits;
        let shrunk = limits.shrunk();
        let scaled = |width: f64| zoom::scaled(cx, width as f32);
        let applied = self.sidebar_split.applied.clone();
        let awaiting = self.sidebar_split.awaiting_read.clone();
        let painted_assistant = self.sidebar_split.assistant.clone();
        let assistant_limits = layout.assistant;
        h_resizable("shell-sidebar-split")
            .with_state(&self.sidebar_split.state)
            .child(
                resizable_panel()
                    .size(scaled(sidebar.width))
                    .size_range(scaled(limits.sidebar_min)..scaled(limits.sidebar_max))
                    .child(navigation),
            )
            .child(
                resizable_panel()
                    .size_range(scaled(limits.workspace_min)..gpui_kit::Pixels::MAX)
                    .child(workspace),
            )
            .on_resize(move |state, _, cx| {
                // The shrunk minimums pin the divider: a drag moved nothing
                // the user chose, so it saves nothing and the split lays
                // out from the sizes it showed.
                if shrunk {
                    applied.set(None);
                    cx.refresh_windows();
                    return;
                }
                let unit = f32::from(zoom::scaled(cx, 1.0));
                let sizes = state.read(cx).sizes();
                let (Some(sidebar), Some(workspace)) = (sizes.first(), sizes.get(1)) else {
                    return;
                };
                let sidebar = f64::from(f32::from(*sidebar) / unit);
                let workspace = f64::from(f32::from(*workspace) / unit);
                // The assistant painted in the workspace this drag measured
                // (GPUI Kit's fixed panel keeps its width), not the one the
                // file names now: Electron may have toggled it since native
                // last loaded the file.
                let assistant = painted_assistant.get().map(|width| {
                    let width = f64::from(f32::from(width) / unit);
                    assistant_limits.map_or(width, |limits| limits.painted(width, workspace))
                });
                let shown = pane_sizes::Shown {
                    sidebar: Some(sidebar),
                    workspace,
                    assistant,
                };
                save_drag(
                    cx,
                    pane_sizes::Divider::Sidebar,
                    shown,
                    || applied.set(None),
                    &awaiting,
                );
            })
            .into_any_element()
    }

    /// The workspace, with the open assistant (its split's limits and
    /// width from [`buddy_core::pane_sizes::layout`]) beside the main pane,
    /// in `row`, its width in pixels at 100%.
    fn render_workspace(
        &mut self,
        assistant: Option<buddy_core::pane_sizes::AssistantLimits>,
        row: buddy_core::pane_sizes::Row,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use buddy_core::pane_sizes;
        use gpui_kit::component::resizable::{h_resizable, resizable_panel};
        let Some(limits) = assistant else {
            self.sidebar_split.assistant.set(None);
            self.sidebar_split.assistant_applied.set(None);
            return self.render_terminal_split(cx);
        };
        let layout = AssistantLayout {
            limits,
            unit: f32::from(zoom::scaled(cx, 1.0)),
            available: row.available,
        };
        if self.sidebar_split.assistant_applied.get() != Some(layout) {
            self.sidebar_split.assistant_state = cx.new(|_| ResizableState::default());
            self.sidebar_split.assistant_applied.set(Some(layout));
        }
        let shrunk = limits.shrunk();
        let scaled = |width: f64| zoom::scaled(cx, width as f32);
        let painted = self.sidebar_split.assistant.clone();
        let assistant_applied = self.sidebar_split.assistant_applied.clone();
        let awaiting = self.sidebar_split.awaiting_read.clone();
        h_resizable("workspace-assistant-split")
            .with_state(&self.sidebar_split.assistant_state)
            .child(
                resizable_panel()
                    .size_range(scaled(limits.main_min)..gpui_kit::Pixels::MAX)
                    .child(self.render_terminal_split(cx)),
            )
            .child(
                resizable_panel()
                    .size(scaled(limits.assistant))
                    .fixed()
                    .size_range(scaled(limits.assistant_min)..scaled(limits.assistant_max))
                    .child(
                        gpui_kit::canvas(
                            move |bounds, _, _| painted.set(Some(bounds.size.width)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .id("assistant-panel")
                            .debug_selector(|| "assistant-panel".into())
                            .size_full()
                            .p_3()
                            .child(self.assistant.clone()),
                    ),
            )
            .on_resize(move |state, _, cx| {
                // Held at its shrunk width, the assistant did not move.
                if shrunk {
                    assistant_applied.set(None);
                    cx.refresh_windows();
                    return;
                }
                let unit = f32::from(zoom::scaled(cx, 1.0));
                let state = state.read(cx);
                let Some(width) = state.sizes().get(1).copied() else {
                    return;
                };
                let workspace = f64::from(f32::from(state.container_size()) / unit);
                // GPUI Kit's state can hold more than the panel paints (a
                // drag past 600 px): the save takes the painted width.
                let width = limits.painted(f64::from(f32::from(width) / unit), workspace);
                // The sidebar beside the workspace did not move.
                let shown = pane_sizes::Shown {
                    sidebar: row.sidebar.then_some(row.available - workspace),
                    workspace,
                    assistant: Some(width),
                };
                save_drag(
                    cx,
                    pane_sizes::Divider::Assistant,
                    shown,
                    || assistant_applied.set(None),
                    &awaiting,
                );
            })
            .into_any_element()
    }

    fn render_terminal_split(&self, cx: &App) -> gpui_kit::AnyElement {
        use gpui_kit::component::resizable::{resizable_panel, v_resizable};
        if !Settings::global(cx).config.ui.terminal_open {
            return self.render_content(cx).into_any_element();
        }
        let height = buddy_core::terminal_panel::panel_height(
            Settings::global(cx).config.ui.terminal_panel_height,
        );
        let panel = self.terminal_panel.downgrade();
        v_resizable("workspace-terminal-split")
            .child(
                resizable_panel()
                    .size_range(zoom::scaled(cx, 100.0)..gpui_kit::Pixels::MAX)
                    .child(self.render_content(cx)),
            )
            .child(
                resizable_panel()
                    .size(zoom::scaled(cx, height as f32))
                    .fixed()
                    .size_range(zoom::scaled(cx, 100.0)..zoom::scaled(cx, 1200.0))
                    .child(self.terminal_panel.clone()),
            )
            .on_resize(move |state, _, cx| {
                if let Some(height) = state.read(cx).sizes().get(1).copied() {
                    let height = buddy_core::terminal_panel::panel_height(f64::from(
                        f32::from(height) / f32::from(zoom::scaled(cx, 1.0)),
                    ));
                    if Settings::update(cx, |config| config.ui.terminal_panel_height = height) {
                        panel.update(cx, |panel, cx| panel.height_changed(cx)).ok();
                    }
                }
            })
            .into_any_element()
    }

    fn render_content(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let container = div().flex_1().min_h_0().w_full().bg(theme.background);
        if self.folder_open {
            return container.child(self.explorer_view.clone());
        }
        if self.tabs.active.is_none() {
            return container.child(
                div()
                    .p_4()
                    .child("No open tabs. Choose a section to open a view."),
            );
        }
        match self.active_section {
            None => container.child(self.dashboard.clone()),
            Some(Section::Settings) => container.child(self.settings_view.clone()),
            Some(Section::GitHub) => container.child(self.github_view.clone()),
            Some(Section::Terminal) => container.child(self.terminal_view.clone()),
            Some(Section::Copilot) => container.child(self.copilot_view.clone()),
            Some(Section::Tasks) => container.child(self.todoist_view.clone()),
            Some(Section::Tempo) => container.child(self.tempo_view.clone()),
            Some(Section::Automation) => container.child(self.automation_view.clone()),
            Some(Section::Ralph) => container.child(self.ralph_view.clone()),
            Some(Section::Crew) => container.child(self.crew_view.clone()),
            Some(Section::Bookmarks) => container.child(self.browser_view.clone()),
            Some(section) => container
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(format!("{} (not yet ported)", section.label())),
        }
    }
}

/// The assistant pane's width at 100%: the third `ui.paneSizes` entry,
/// within the pane's 280-600 limits.
impl Render for BuddyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_open = Settings::global(cx).config.ui.terminal_open;
        self.terminal_panel
            .update(cx, |panel, cx| panel.set_visible(terminal_open, cx));
        let assistant_open = Settings::global(cx).config.ui.assistant_open;
        if assistant_open {
            let context = self.assistant_context(cx);
            self.assistant
                .update(cx, |view, cx| view.set_sidebar_context(context, cx));
        }
        let sidebar_split = self.render_sidebar_split(window, cx);
        let theme = cx.theme();
        let font_family = theme.font_family.clone();
        let background = theme.background;
        let foreground = theme.foreground;

        let shell = v_flex();
        #[cfg(target_os = "macos")]
        let shell = crate::macos_menu::window_actions(shell);
        let content =
            shell
                .key_context("BuddyApp")
                .on_action(cx.listener(|this, _: &NextGlobalTab, window, cx| {
                    this.tabs.cycle(true);
                    this.restore_tab(window, cx);
                }))
                .on_action(cx.listener(|this, _: &PreviousGlobalTab, window, cx| {
                    this.tabs.cycle(false);
                    this.restore_tab(window, cx);
                }))
                .on_action(cx.listener(|this, _: &CloseGlobalTab, window, cx| {
                    if let Some(key) = this.tabs.active.clone() {
                        this.tabs.close(&key);
                    }
                    this.restore_tab(window, cx);
                }))
                .on_action(cx.listener(|this, _: &NavigationBack, window, cx| {
                    this.tabs.travel(false);
                    this.restore_tab(window, cx);
                }))
                .on_action(cx.listener(|this, _: &NavigationForward, window, cx| {
                    this.tabs.travel(true);
                    this.restore_tab(window, cx);
                }))
                .on_action(cx.listener(Self::open_folder))
                .on_action(
                    cx.listener(|this, _: &crate::browser_view::ZoomPageIn, _, cx| {
                        if !this
                            .browser_view
                            .update(cx, |view, cx| view.zoom_shortcut(true, cx))
                        {
                            cx.propagate();
                        }
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &crate::browser_view::ZoomPageOut, _, cx| {
                        if !this
                            .browser_view
                            .update(cx, |view, cx| view.zoom_shortcut(false, cx))
                        {
                            cx.propagate();
                        }
                    }),
                )
                .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                    this.toggle_terminal(window, cx)
                }))
                .on_action(|_: &ToggleAssistant, _, cx| Self::toggle_assistant(cx))
                .track_focus(&self.focus)
                .size_full()
                .font_family(font_family)
                .text_size(zpx(13.0))
                .bg(background)
                .text_color(foreground)
                .child(title_bar::render(self, window, cx))
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(activity_bar::render(self, cx))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(tab_bar::render(self, window, cx))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_h_0()
                                        .w_full()
                                        .child(sidebar_split),
                                ),
                        ),
                )
                .child(status_bar::render(self, cx))
                .child(zoom::wheel_listener());
        crate::settings::sound_probe::observe("app-root", content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("shell/tab_bar_tests.rs");

    // Each mouse test has its own process/profile: section persistence must
    // never write the developer's settings or race another GPUI test's env.
    fn status_click_child(test: &str) -> bool {
        if std::env::var("BUDDY_STATUS_CLICK_CHILD").as_deref() == Ok(test) {
            return false;
        }
        let profile = tempfile::tempdir().unwrap();
        let config = profile.path().join("config.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&buddy_core::config::AppConfig::default()).unwrap(),
        )
        .unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--nocapture"])
            .env("BUDDY_STATUS_CLICK_CHILD", test)
            .env("BUDDY_CONFIG_PATH", &config)
            // No backend is needed to prove the real navigation callbacks.
            .env("BUDDY_CONVEX_URL", "fixture://status-click-offline");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    fn status_click_shell(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (gpui_kit::AnyWindowHandle, Entity<BuddyApp>) {
        test_shell(
            cx,
            buddy_core::config::AppConfig::default(),
            Default::default(),
            None,
        )
    }

    fn test_shell(
        cx: &mut gpui_kit::TestAppContext,
        config: buddy_core::config::AppConfig,
        options: gpui_kit::WindowOptions,
        section: Option<Section>,
    ) -> (gpui_kit::AnyWindowHandle, Entity<BuddyApp>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            cx.set_global(Runtime::for_test(cx.background_executor().clone()));
            gpui_kit::open_window(options, cx, |window, cx| {
                let dashboard = cx.new(|cx| DashboardView::new(window, cx, false));
                cx.new(|cx| {
                    BuddyApp::observe_settings(window, cx);
                    BuddyApp {
                        active_section: section,
                        active_account: None,
                        dashboard,
                        settings_view: cx.new(|cx| SettingsView::new(window, cx)),
                        github_view: cx.new(|cx| GitHubView::new(window, cx)),
                        terminal_view: cx.new(|cx| TerminalView::new(window, cx)),
                        terminal_panel: cx
                            .new(|cx| crate::terminal_view::panel::TerminalPanel::new(window, cx)),
                        copilot_view: cx.new(|cx| CopilotView::new(window, cx)),
                        assistant: cx
                            .new(|cx| crate::copilot_view::chat::ChatView::new(window, cx)),
                        todoist_view: cx.new(|cx| TodoistView::new(window, cx)),
                        tempo_view: cx.new(|cx| TempoView::new(window, cx)),
                        automation_view: cx.new(|cx| AutomationView::new(window, cx)),
                        ralph_view: cx.new(|cx| RalphView::new(window, cx)),
                        crew_view: cx.new(|cx| CrewView::new(window, cx)),
                        bookmarks_view: cx.new(|cx| BookmarksView::new(window, cx)),
                        browser_view: cx.new(|cx| BrowserView::new(window, cx)),
                        explorer_view: cx.new(|_| crate::explorer_view::ExplorerView::new()),
                        folder_open: false,
                        window_state: WindowPersistence::new(),
                        focus: BuddyApp::initial_focus(window, cx),
                        tabs: crate::shell::navigation::Tabs::default(),
                        sidebar_split: SidebarSplit::new(cx),
                    }
                })
            })
            .unwrap()
        })
    }

    /// A shell on the GitHub section in a `width` x 800 window, after
    /// saving `config` (a drag's save edits the file, so it starts there).
    fn sidebar_shell(
        cx: &mut gpui_kit::TestAppContext,
        mut config: buddy_core::config::AppConfig,
        width: f32,
    ) -> gpui_kit::VisualTestContext {
        use gpui_kit::{point, px, size};
        config.native.active_section = Some("github".into());
        config.save().unwrap();
        let options = gpui_kit::WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(width), px(800.0)),
            })),
            ..Default::default()
        };
        let (window, _shell) = test_shell(cx, config, options, Some(Section::GitHub));
        gpui_kit::VisualTestContext::from_window(window, cx)
    }

    /// The sidebar list's first button, which spans the sidebar less its
    /// padding (8px at 100%).
    fn sidebar_item(
        visual: &mut gpui_kit::VisualTestContext,
    ) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        use gpui_kit::test::TestWindowExt as _;
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("navigation-item", 0usize)).bounds()
        })
    }

    fn element_bounds(
        visual: &mut gpui_kit::VisualTestContext,
        id: &'static str,
    ) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        use gpui_kit::test::TestWindowExt as _;
        visual.update(|window, cx| window.render_frame(cx));
        visual
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is not laid out"))
    }

    /// Drag the sidebar's divider, beside `item` (see [`sidebar_item`]),
    /// to `to`. Dispatched directly: `simulate_*` would run the
    /// dashboard's queued network work, which needs a Tokio runtime.
    fn drag_sidebar_divider(
        visual: &mut gpui_kit::VisualTestContext,
        item: gpui_kit::Bounds<gpui_kit::Pixels>,
        padding: f32,
        to: gpui_kit::Pixels,
    ) {
        let boundary = item.right() + gpui_kit::px(padding);
        drag_divider(visual, boundary, item.center().y, to);
    }

    /// Drag the divider of the open assistant, at its left edge, to `to`.
    fn drag_assistant_divider(visual: &mut gpui_kit::VisualTestContext, to: gpui_kit::Pixels) {
        let assistant = element_bounds(visual, "assistant-panel");
        drag_divider(visual, assistant.left(), assistant.center().y, to);
    }

    /// Drag the divider at `boundary` (its handle straddles it) to `to`.
    fn drag_divider(
        visual: &mut gpui_kit::VisualTestContext,
        boundary: gpui_kit::Pixels,
        y: gpui_kit::Pixels,
        to: gpui_kit::Pixels,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{
            MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, point, px,
        };
        visual.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(boundary - px(2.0), y),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            for x in [boundary + px(10.0), to] {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(x, y),
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    }),
                    cx,
                );
                window.render_frame(cx);
            }
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(to, y),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        });
    }

    fn sidebar_width(visual: &mut gpui_kit::VisualTestContext, padding: f32) -> f32 {
        f32::from(sidebar_item(visual).size.width) + 2.0 * padding
    }

    fn saved_pane_sizes(visual: &mut gpui_kit::VisualTestContext) -> Vec<f64> {
        let saved = visual.update(|_, cx| Settings::global(cx).config.ui.pane_sizes.clone());
        let disk = buddy_core::config::AppConfig::load().unwrap();
        assert_eq!(disk.ui.pane_sizes, saved);
        saved
    }

    #[track_caller]
    fn assert_sizes(saved: &[f64], expected: &[f64]) {
        assert!(
            saved.len() == expected.len()
                && saved
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (a - b).abs() <= 1.0),
            "{saved:?} != {expected:?}"
        );
    }

    /// Electron's `assistant:send-prompt` reaches the assistant, which native
    /// opens first. No account is known here, so the turn cannot start: the
    /// prompt and its model wait in the assistant, as after any refused send.
    #[gpui_kit::test]
    fn a_sent_prompt_opens_the_assistant_with_the_prompt_and_its_model(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_sent_prompt_opens_the_assistant_with_the_prompt_and_its_model",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.native.active_section = Some("github".into());
        config.save().unwrap();
        let (window, shell) = test_shell(cx, config, Default::default(), Some(Section::GitHub));
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let prompt = crate::copilot_view::chat::AssistantPrompt {
            prompt: "Address the unresolved review comments on PR #7".into(),
            model: Some("claude-opus-4.6".into()),
            context: Some("Thread context".into()),
            context_label: Some("1 unresolved review thread".into()),
        };
        visual.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.send_assistant_prompt(prompt, window, cx)
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.update(|_, cx| {
            assert!(Settings::global(cx).config.ui.assistant_open);
            assert!(
                buddy_core::config::AppConfig::load()
                    .unwrap()
                    .ui
                    .assistant_open
            );
            let assistant = shell.read(cx).assistant.read(cx);
            assert_eq!(
                assistant.injected_state(cx),
                (
                    "Address the unresolved review comments on PR #7".into(),
                    Some(Some("claude-opus-4.6".into()))
                )
            );
        });
        assert!(element_bounds(&mut visual, "assistant-panel").size.width > gpui_kit::px(0.0));
    }

    #[gpui_kit::test]
    fn sidebar_takes_its_saved_share_and_a_drag_saves_both_widths(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::sidebar_takes_its_saved_share_and_a_drag_saves_both_widths",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0];
        // The row beside the 48px activity bar is 1200px wide.
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        let sidebar = f32::from(before.size.width) + 16.0;
        assert!((sidebar - 400.0).abs() <= 1.0, "sidebar width: {sidebar}");
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        let after = sidebar_width(&mut visual, 8.0);
        assert!((after - 300.0).abs() <= 1.0, "dragged width: {after}");
        let saved = saved_pane_sizes(&mut visual);
        assert_eq!(saved.len(), 3, "{saved:?}");
        assert!((saved[0] - 300.0).abs() <= 1.0, "{saved:?}");
        assert!((saved[1] - 900.0).abs() <= 1.0, "{saved:?}");
        assert_eq!(saved[2], 350.0);
    }

    #[gpui_kit::test]
    fn sidebar_follows_pane_sizes_that_change_underneath_it(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::sidebar_follows_pane_sizes_that_change_underneath_it") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0];
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let initial = sidebar_width(&mut visual, 8.0);
        // Reload Configuration after the file's sizes changed.
        let mut edited = buddy_core::config::AppConfig::load().unwrap();
        edited.ui.pane_sizes = vec![300.0, 900.0];
        edited.save().unwrap();
        visual.update(|_, cx| {
            let config = buddy_core::config::AppConfig::load().unwrap();
            crate::theme::install(&config, cx);
            Settings::replace(cx, config);
            cx.refresh_windows();
        });
        let reloaded = sidebar_width(&mut visual, 8.0);
        // Electron writes new sizes; native's next save reloads the file.
        let mut electron = buddy_core::config::AppConfig::load().unwrap();
        electron.ui.pane_sizes = vec![500.0, 700.0, 350.0];
        electron.save().unwrap();
        visual.update(|_, cx| {
            assert!(Settings::update(cx, |config| config.ui.terminal_open = false));
        });
        let electron_changed = sidebar_width(&mut visual, 8.0);
        assert!((initial - 400.0).abs() <= 1.0, "initial: {initial}");
        assert!((reloaded - 300.0).abs() <= 1.0, "after Reload: {reloaded}");
        assert!(
            (electron_changed - 500.0).abs() <= 1.0,
            "after Electron's write: {electron_changed}"
        );
    }

    #[gpui_kit::test]
    fn sidebar_split_fits_a_narrow_window_at_150_percent(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::sidebar_split_fits_a_narrow_window_at_150_percent") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.zoom_level = 150.0;
        let mut visual = sidebar_shell(cx, config, 900.0);
        visual.update(|_, cx| {
            crate::zoom::init(cx);
            cx.refresh_windows();
        });
        // 552px at 100% beside the 72px activity bar: the 200 and 400
        // minimums shrink to 184 and 368, 276 and 552 window pixels.
        let sidebar = sidebar_width(&mut visual, 12.0);
        let workspace = element_bounds(&mut visual, "shell-workspace");
        assert!((sidebar - 276.0).abs() <= 1.0, "sidebar: {sidebar}");
        assert!(
            f32::from(workspace.right()) <= 900.5,
            "workspace overflows: {workspace:?}"
        );
        assert!(
            (f32::from(workspace.size.width) - 552.0).abs() <= 1.0,
            "workspace: {workspace:?}"
        );
    }

    #[gpui_kit::test]
    fn an_open_assistant_bounds_a_sidebar_drag_and_the_save_keeps_the_assistant(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::an_open_assistant_bounds_a_sidebar_drag_and_the_save_keeps_the_assistant",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![300.0, 900.0, 600.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        // 300 of 1,800 saved: a sixth of the 1,200px row, and the
        // assistant's third is 400.
        let before = sidebar_item(&mut visual);
        let initial = f32::from(before.size.width) + 16.0;
        assert!((initial - 200.0).abs() <= 1.0, "initial: {initial}");
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
        // Far right: the workspace keeps the assistant and the main 250.
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 1000.0));
        let dragged = sidebar_width(&mut visual, 8.0);
        let workspace = element_bounds(&mut visual, "shell-workspace");
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((dragged - 550.0).abs() <= 1.0, "dragged: {dragged}");
        assert!(
            (f32::from(workspace.size.width) - 650.0).abs() <= 1.0,
            "workspace: {workspace:?}"
        );
        assert!(
            f32::from(assistant.right()) <= 1248.5,
            "assistant overflows: {assistant:?}"
        );
        // The widths shown are saved (I2), so the save lays out at the
        // same 550 and 400 (I5).
        let saved = saved_pane_sizes(&mut visual);
        assert_sizes(&saved, &[550.0, 250.0, 400.0]);
        let after = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((after - 550.0).abs() <= 1.0, "after the save: {after}");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
    }

    #[gpui_kit::test]
    fn a_sidebar_drag_saves_the_content_it_measured_when_electron_opened_the_assistant(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_sidebar_drag_saves_the_content_it_measured_when_electron_opened_the_assistant",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0];
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        // Electron opens the assistant in the shared file; native has not
        // loaded it, so the drag measures a workspace with no assistant.
        let mut electron = buddy_core::config::AppConfig::load().unwrap();
        electron.ui.assistant_open = true;
        electron.save().unwrap();
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        let saved = saved_pane_sizes(&mut visual);
        let disk = buddy_core::config::AppConfig::load().unwrap();
        assert!(disk.ui.assistant_open, "Electron's toggle is kept");
        assert_eq!(saved.len(), 3, "{saved:?}");
        assert!((saved[0] - 300.0).abs() <= 1.0, "{saved:?}");
        // The whole 900px workspace, not 900 less the assistant's 350.
        assert!((saved[1] - 900.0).abs() <= 1.0, "{saved:?}");
        assert_eq!(saved[2], 350.0);
        // The split now lays out from the file, with Electron's open
        // assistant, as a restart would: 300 of 1,550 of the row.
        let after = sidebar_width(&mut visual, 8.0);
        let restart = 300.0 / 1550.0 * 1200.0;
        assert!((after - restart).abs() <= 1.0, "after the save: {after}");
    }

    /// Reload Configuration as the Reload action does.
    fn reload_configuration(visual: &mut gpui_kit::VisualTestContext) {
        visual.update(|_, cx| {
            let config = buddy_core::config::AppConfig::load().unwrap();
            crate::theme::install(&config, cx);
            Settings::replace(cx, config);
            cx.refresh_windows();
        });
    }

    #[gpui_kit::test]
    fn an_open_assistant_fits_a_narrow_window_at_150_percent(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::an_open_assistant_fits_a_narrow_window_at_150_percent") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.zoom_level = 150.0;
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 900.0);
        visual.update(|_, cx| {
            crate::zoom::init(cx);
            cx.refresh_windows();
        });
        // 552px at 100% beside the activity bar: the assistant's share of
        // the default [300, 900, 350] is held at its 280 minimum, and 200 +
        // 280 + 250 do not fit, so the minimums shrink by 552/730: the
        // sidebar's 200 becomes 151.2 (226.8 window pixels), the
        // workspace's 530 becomes 400.8 (601.2) and the assistant's 280
        // becomes 211.7 (317.6).
        let scale = 552.0 / 730.0;
        let sidebar = sidebar_width(&mut visual, 12.0);
        let workspace = element_bounds(&mut visual, "shell-workspace");
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (sidebar - 200.0 * scale * 1.5).abs() <= 1.0,
            "sidebar: {sidebar}"
        );
        assert!(
            (f32::from(workspace.size.width) - 530.0 * scale * 1.5).abs() <= 1.0,
            "workspace: {workspace:?}"
        );
        assert!(
            f32::from(workspace.right()) <= 900.5,
            "workspace overflows: {workspace:?}"
        );
        assert!(
            f32::from(assistant.right()) <= 900.5,
            "assistant overflows: {assistant:?}"
        );
        assert!(
            (f32::from(assistant.size.width) - 280.0 * scale * 1.5).abs() <= 1.0,
            "assistant: {assistant:?}"
        );
    }

    #[gpui_kit::test]
    fn an_assistant_only_change_on_reload_moves_the_divider(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::an_assistant_only_change_on_reload_moves_the_divider") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0, 350.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        // 400 of 1,550 saved: of the 1,200px row; the assistant's share
        // (271) is held at its 280 minimum.
        let initial = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (initial - 400.0 / 1550.0 * 1200.0).abs() <= 1.0,
            "initial: {initial}"
        );
        assert!(
            (f32::from(assistant.size.width) - 280.0).abs() <= 1.0,
            "{assistant:?}"
        );
        let mut edited = buddy_core::config::AppConfig::load().unwrap();
        edited.ui.pane_sizes = vec![400.0, 800.0, 600.0];
        edited.save().unwrap();
        reload_configuration(&mut visual);
        // 400 of 1,800 now, and the assistant takes its third: 400.
        let reloaded = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (reloaded - 400.0 / 1800.0 * 1200.0).abs() <= 1.0,
            "after Reload: {reloaded}"
        );
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
        assert!(f32::from(assistant.right()) <= 1248.5, "{assistant:?}");
        // Closing the assistant keeps the divider where it is: the content
        // takes back the assistant's weight in the file, so a restart does
        // too, and the assistant's entry becomes the 400 it painted.
        // Opening it again restores the sizes (I3).
        visual.update(|_, cx| BuddyApp::toggle_assistant(cx));
        let closed = sidebar_width(&mut visual, 8.0);
        assert!((closed - reloaded).abs() <= 1.0, "closed: {closed}");
        assert_sizes(&saved_pane_sizes(&mut visual), &[400.0, 1400.0, 400.0]);
        visual.update(|_, cx| BuddyApp::toggle_assistant(cx));
        let reopened = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((reopened - reloaded).abs() <= 1.0, "reopened: {reopened}");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
        assert_sizes(&saved_pane_sizes(&mut visual), &[400.0, 800.0, 600.0]);
    }

    /// Review round 4, finding 2: `[300, 400, 350]` in a 1,600px row.
    #[gpui_kit::test]
    fn opening_the_assistant_leaves_the_sidebar_where_it_was(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::opening_the_assistant_leaves_the_sidebar_where_it_was") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![300.0, 400.0, 350.0];
        let mut visual = sidebar_shell(cx, config, 1648.0);
        let share = 300.0 / 700.0 * 1600.0;
        let closed = sidebar_width(&mut visual, 8.0);
        assert!((closed - share).abs() <= 1.0, "closed: {closed}");
        // Open: the divider stays at 686 and the assistant opens at the
        // 350 its entry names, beside a main pane wider than 250.
        visual.update(|_, cx| BuddyApp::toggle_assistant(cx));
        let open = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((open - share).abs() <= 1.0, "open: {open}");
        assert!(
            (f32::from(assistant.size.width) - 350.0).abs() <= 1.0,
            "{assistant:?}"
        );
        let saved = saved_pane_sizes(&mut visual);
        assert_eq!(saved[0], 300.0);
        assert!((saved[1] + saved[2] - 400.0).abs() <= 1e-6, "{saved:?}");
        // A restart lays out the same.
        reload_configuration(&mut visual);
        let restarted = sidebar_width(&mut visual, 8.0);
        assert!((restarted - share).abs() <= 1.0, "restarted: {restarted}");
        // Close: the file is back where it started.
        visual.update(|_, cx| BuddyApp::toggle_assistant(cx));
        let again = sidebar_width(&mut visual, 8.0);
        assert!((again - share).abs() <= 1.0, "closed again: {again}");
        assert_sizes(&saved_pane_sizes(&mut visual), &[300.0, 400.0, 350.0]);
    }

    #[gpui_kit::test]
    fn widening_the_window_lays_out_the_saved_share_not_the_earlier_clamp(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::widening_the_window_lays_out_the_saved_share_not_the_earlier_clamp",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![450.0, 450.0, 600.0];
        config.ui.assistant_open = true;
        // An 800px row: the assistant's 40% (320) and the main pane's 250
        // leave the sidebar 230, so its 30% (240) is held there.
        let mut visual = sidebar_shell(cx, config, 848.0);
        let narrow = sidebar_width(&mut visual, 8.0);
        assert!((narrow - 230.0).abs() <= 1.0, "narrow: {narrow}");
        // A 1,600px row holds the saved 30%: 480, as a launch at that
        // width lays out, not the earlier 230/800 share (460).
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1648.0), gpui_kit::px(800.0)));
        element_bounds(&mut visual, "shell-workspace");
        let wide = sidebar_width(&mut visual, 8.0);
        assert!((wide - 480.0).abs() <= 1.0, "after widening: {wide}");
        // The assistant's 40% (640) paints its 600 maximum.
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (f32::from(assistant.size.width) - 600.0).abs() <= 1.0,
            "{assistant:?}"
        );
        assert!(f32::from(assistant.right()) <= 1648.5, "{assistant:?}");
        // Back to the narrow row, and the clamp returns.
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(848.0), gpui_kit::px(800.0)));
        element_bounds(&mut visual, "shell-workspace");
        let again = sidebar_width(&mut visual, 8.0);
        assert!((again - 230.0).abs() <= 1.0, "narrow again: {again}");
    }

    #[gpui_kit::test]
    fn an_assistant_drag_past_its_cap_saves_the_width_it_shows(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::an_assistant_drag_past_its_cap_saves_the_width_it_shows")
        {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![300.0, 900.0, 350.0];
        config.ui.assistant_open = true;
        // 300 of 1,550 of the 1,600px row.
        let mut visual = sidebar_shell(cx, config, 1648.0);
        let share = 300.0 / 1550.0 * 1600.0;
        let initial = sidebar_width(&mut visual, 8.0);
        assert!((initial - share).abs() <= 1.0, "initial: {initial}");
        // Far left: GPUI Kit's state gives the assistant about 1,040px,
        // and the panel paints its 600 maximum.
        let item = sidebar_item(&mut visual);
        drag_assistant_divider(&mut visual, item.right() + gpui_kit::px(100.0));
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (f32::from(assistant.size.width) - 600.0).abs() <= 1.0,
            "{assistant:?}"
        );
        // The file holds the widths shown: the sidebar where it was, and
        // the 600 the assistant paints.
        let saved = saved_pane_sizes(&mut visual);
        let px = f64::from(share);
        assert_sizes(&saved, &[px, 1600.0 - px - 600.0, 600.0]);
        let after = sidebar_width(&mut visual, 8.0);
        assert!((after - share).abs() <= 1.0, "after the drag: {after}");
        // A restart (Reload from the file) lays out the same.
        reload_configuration(&mut visual);
        let restarted = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((restarted - share).abs() <= 1.0, "restarted: {restarted}");
        assert!(
            (f32::from(assistant.size.width) - 600.0).abs() <= 1.0,
            "{assistant:?}"
        );
    }

    /// Review round 4, finding 1: `[300, 400, 280]` in a 1,600px row, the
    /// assistant dragged to 600.
    #[gpui_kit::test]
    fn an_assistant_drag_leaves_the_sidebar_where_it_was(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::an_assistant_drag_leaves_the_sidebar_where_it_was") {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![300.0, 400.0, 280.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1648.0);
        let share = 300.0 / 980.0 * 1600.0;
        let initial = sidebar_width(&mut visual, 8.0);
        assert!((initial - share).abs() <= 1.0, "initial: {initial}");
        drag_assistant_divider(&mut visual, gpui_kit::px(1648.0 - 600.0));
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (f32::from(assistant.size.width) - 600.0).abs() <= 1.0,
            "{assistant:?}"
        );
        // The widths shown, with the sidebar where it was.
        let saved = saved_pane_sizes(&mut visual);
        let px = f64::from(share);
        assert_sizes(&saved, &[px, 1600.0 - px - 600.0, 600.0]);
        let after = sidebar_width(&mut visual, 8.0);
        assert!((after - share).abs() <= 1.0, "after the drag: {after}");
        reload_configuration(&mut visual);
        let restarted = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((restarted - share).abs() <= 1.0, "restarted: {restarted}");
        assert!(
            (f32::from(assistant.size.width) - 600.0).abs() <= 1.0,
            "{assistant:?}"
        );
    }

    #[gpui_kit::test]
    fn a_sidebar_drag_keeps_the_assistant_that_painted_during_a_fallback(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_sidebar_drag_keeps_the_assistant_that_painted_during_a_fallback",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        // The zero falls back to the default sidebar and content, but the
        // third entry is a valid 600 weight: a third of the 1,800.
        config.ui.pane_sizes = vec![0.0, 900.0, 600.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        let initial = f32::from(before.size.width) + 16.0;
        assert!((initial - 200.0).abs() <= 1.0, "initial: {initial}");
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        // The 400 the assistant painted is saved, not the default 350.
        let saved = saved_pane_sizes(&mut visual);
        assert_sizes(&saved, &[300.0, 500.0, 400.0]);
        let after = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((after - 300.0).abs() <= 1.0, "after the drag: {after}");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
    }

    #[gpui_kit::test]
    fn a_narrow_launch_lays_out_the_saved_share_once_the_window_widens(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_narrow_launch_lays_out_the_saved_share_once_the_window_widens",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.zoom_level = 150.0;
        let mut visual = sidebar_shell(cx, config, 900.0);
        visual.update(|_, cx| {
            crate::zoom::init(cx);
            cx.refresh_windows();
        });
        // The shrunk 184 minimum, a third of the row, not the saved quarter.
        let narrow = sidebar_width(&mut visual, 12.0);
        assert!((narrow - 276.0).abs() <= 1.0, "narrow: {narrow}");
        // 800px at 100% beside the activity bar holds both minimums: the
        // saved 300 of 1,200 is 200, 300 window pixels.
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1272.0), gpui_kit::px(800.0)));
        element_bounds(&mut visual, "shell-workspace");
        let wide = sidebar_width(&mut visual, 12.0);
        assert!((wide - 300.0).abs() <= 1.0, "after widening: {wide}");
        let workspace = element_bounds(&mut visual, "shell-workspace");
        assert!(f32::from(workspace.right()) <= 1272.5, "{workspace:?}");
    }

    /// Review round 5, finding 1: Electron closes the assistant while
    /// native drags it. `[300, 400, 280]` in a 1,600px row, dragged to 600.
    #[gpui_kit::test]
    fn an_assistant_drag_after_electron_closed_it_keeps_the_closed_layout(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::an_assistant_drag_after_electron_closed_it_keeps_the_closed_layout",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![300.0, 400.0, 280.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1648.0);
        element_bounds(&mut visual, "assistant-panel");
        let mut electron = buddy_core::config::AppConfig::load().unwrap();
        electron.ui.assistant_open = false;
        electron.save().unwrap();
        drag_assistant_divider(&mut visual, gpui_kit::px(1648.0 - 600.0));
        // Nothing was saved over the closed layout, which shows the sidebar
        // at 300 of 700 of the row, not 784.
        let saved = saved_pane_sizes(&mut visual);
        assert_eq!(saved, vec![300.0, 400.0, 280.0]);
        assert!(
            !buddy_core::config::AppConfig::load()
                .unwrap()
                .ui
                .assistant_open
        );
        let closed = sidebar_width(&mut visual, 8.0);
        let share = 300.0 / 700.0 * 1600.0;
        assert!((closed - share).abs() <= 1.0, "closed: {closed}");
    }

    /// Review round 5, finding 2: `[900, 300, 600]` in a 1,200px row holds
    /// the sidebar at its 550 maximum beside a 400px assistant, dragged to
    /// 300.
    #[gpui_kit::test]
    fn an_assistant_drag_beside_a_sidebar_at_its_maximum_moves_only_the_assistant(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::an_assistant_drag_beside_a_sidebar_at_its_maximum_moves_only_the_assistant",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![900.0, 300.0, 600.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let initial = sidebar_width(&mut visual, 8.0);
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((initial - 550.0).abs() <= 1.0, "initial: {initial}");
        assert!(
            (f32::from(assistant.size.width) - 400.0).abs() <= 1.0,
            "{assistant:?}"
        );
        drag_assistant_divider(&mut visual, gpui_kit::px(1248.0 - 300.0));
        assert_sizes(&saved_pane_sizes(&mut visual), &[550.0, 350.0, 300.0]);
        for when in ["after the release", "after Reload"] {
            let sidebar = sidebar_width(&mut visual, 8.0);
            let assistant = element_bounds(&mut visual, "assistant-panel");
            assert!((sidebar - 550.0).abs() <= 1.0, "{when}: {sidebar}");
            assert!(
                (f32::from(assistant.size.width) - 300.0).abs() <= 1.0,
                "{when}: {assistant:?}"
            );
            reload_configuration(&mut visual);
        }
    }

    /// Review round 5, finding 3: `[100, 600, 300]` in a 1,200px row holds
    /// the sidebar at its 200 minimum beside a 333px assistant, and the
    /// sidebar is dragged to 300.
    #[gpui_kit::test]
    fn a_sidebar_drag_off_its_minimum_leaves_the_assistant_as_it_was_shown(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_sidebar_drag_off_its_minimum_leaves_the_assistant_as_it_was_shown",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![100.0, 600.0, 300.0];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        let initial = f32::from(before.size.width) + 16.0;
        let assistant = element_bounds(&mut visual, "assistant-panel");
        assert!((initial - 200.0).abs() <= 1.0, "initial: {initial}");
        assert!(
            (f32::from(assistant.size.width) - 1000.0 / 3.0).abs() <= 1.0,
            "{assistant:?}"
        );
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        assert_sizes(
            &saved_pane_sizes(&mut visual),
            &[300.0, 900.0 - 1000.0 / 3.0, 1000.0 / 3.0],
        );
        for when in ["after the release", "after Reload"] {
            let sidebar = sidebar_width(&mut visual, 8.0);
            let shown = element_bounds(&mut visual, "assistant-panel");
            assert!((sidebar - 300.0).abs() <= 1.0, "{when}: {sidebar}");
            assert!(
                (f32::from(shown.size.width) - f32::from(assistant.size.width)).abs() <= 1.0,
                "{when}: {shown:?}"
            );
            reload_configuration(&mut visual);
        }
    }

    /// Review round 5, finding 5: hand-edited weights near `f64::MAX` save
    /// finite widths that both apps can read back.
    #[gpui_kit::test]
    fn an_assistant_drag_over_huge_weights_saves_a_readable_file(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::an_assistant_drag_over_huge_weights_saves_a_readable_file",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![3e306, 9e306, 3e306];
        config.ui.assistant_open = true;
        let mut visual = sidebar_shell(cx, config, 1248.0);
        // A fifth of the row, beside the assistant at its 280 minimum.
        let initial = sidebar_width(&mut visual, 8.0);
        assert!((initial - 240.0).abs() <= 1.0, "initial: {initial}");
        drag_assistant_divider(&mut visual, gpui_kit::px(1248.0 - 400.0));
        let saved = saved_pane_sizes(&mut visual);
        assert_sizes(&saved, &[240.0, 560.0, 400.0]);
        let path = buddy_core::config::config_path().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let written = body["ui"]["paneSizes"].as_array().unwrap();
        assert!(written.iter().all(serde_json::Value::is_f64), "{written:?}");
    }

    #[gpui_kit::test]
    fn a_drag_whose_save_cannot_read_the_file_stays_until_a_later_read(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_drag_whose_save_cannot_read_the_file_stays_until_a_later_read",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0];
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        // A file the save cannot read back fails the save.
        let path = buddy_core::config::config_path().unwrap();
        std::fs::write(&path, b"{").unwrap();
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        // Nothing on disk says where the divider is, so the drag stays
        // rather than the sizes native last read (I4).
        let after = sidebar_width(&mut visual, 8.0);
        assert!(
            (after - 300.0).abs() <= 1.0,
            "after the failed save: {after}"
        );
        // Review round 5, finding 4: a window resize lays out from the
        // widths the drag showed, [300, 900], not the [400, 800] in memory.
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1648.0), gpui_kit::px(800.0)));
        element_bounds(&mut visual, "shell-workspace");
        let wide = sidebar_width(&mut visual, 8.0);
        assert!((wide - 400.0).abs() <= 1.0, "after widening: {wide}");
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1248.0), gpui_kit::px(800.0)));
        element_bounds(&mut visual, "shell-workspace");
        let back = sidebar_width(&mut visual, 8.0);
        assert!(
            (back - 300.0).abs() <= 1.0,
            "back to the drag's row: {back}"
        );
        visual.update(|_, cx| {
            let settings = Settings::global(cx);
            assert!(settings.save_error.is_some(), "the error stays on screen");
            assert_eq!(settings.config.ui.pane_sizes, vec![400.0, 800.0]);
        });
        assert_eq!(std::fs::read(&path).unwrap(), b"{");
        // The file is readable again, with Electron's sizes, and the next
        // save reads it: the split lays out from the file.
        let mut electron = buddy_core::config::AppConfig::default();
        electron.native.active_section = Some("github".into());
        electron.ui.pane_sizes = vec![500.0, 700.0];
        electron.save().unwrap();
        visual.update(|_, cx| {
            assert!(Settings::update(cx, |config| config.ui.terminal_open = false));
        });
        let reread = sidebar_width(&mut visual, 8.0);
        assert!((reread - 500.0).abs() <= 1.0, "after the read: {reread}");
    }

    /// Review round 4, finding 3: Electron holds the lock with new sizes
    /// on disk when a native drag saves.
    #[gpui_kit::test]
    fn a_drag_refused_by_the_lock_lays_out_the_file_not_the_sizes_in_memory(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::a_drag_refused_by_the_lock_lays_out_the_file_not_the_sizes_in_memory",
        ) {
            return;
        }
        let mut config = buddy_core::config::AppConfig::default();
        config.ui.pane_sizes = vec![400.0, 800.0];
        let mut visual = sidebar_shell(cx, config, 1248.0);
        let before = sidebar_item(&mut visual);
        let initial = f32::from(before.size.width) + 16.0;
        assert!((initial - 400.0).abs() <= 1.0, "initial: {initial}");
        let path = buddy_core::config::config_path().unwrap();
        let mut electron = buddy_core::config::AppConfig::load().unwrap();
        electron.ui.pane_sizes = vec![500.0, 700.0];
        electron.save().unwrap();
        let lock = buddy_core::config_lock::ConfigLock::acquire(&path, std::time::Duration::ZERO)
            .unwrap()
            .unwrap();
        drag_sidebar_divider(&mut visual, before, 8.0, gpui_kit::px(48.0 + 300.0));
        // Not the 400 native had in memory, nor the unsaved 300: the 500
        // the file holds, as a restart would show.
        let after = sidebar_width(&mut visual, 8.0);
        assert!((after - 500.0).abs() <= 1.0, "after the refusal: {after}");
        visual.update(|_, cx| {
            let settings = Settings::global(cx);
            assert!(
                settings.save_error.as_deref().unwrap().contains("busy"),
                "{:?}",
                settings.save_error
            );
            assert_eq!(settings.config.ui.pane_sizes, vec![500.0, 700.0]);
        });
        assert!(lock.is_held());
        drop(lock);
        assert_eq!(
            buddy_core::config::AppConfig::load().unwrap().ui.pane_sizes,
            vec![500.0, 700.0]
        );
    }

    #[gpui_kit::test]
    fn status_prs_click_selects_my_prs_and_clears_retained_page(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child(
            "app::tests::status_prs_click_selects_my_prs_and_clears_retained_page",
        ) {
            return;
        }
        use gpui_kit::test::TestWindowExt as _;
        let (window, shell) = status_click_shell(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        let retained = visual.update(|_, cx| shell.read(cx).github_view.entity_id());
        for _ in 0..2 {
            visual.update(|window, cx| {
                shell
                    .read(cx)
                    .github_view
                    .clone()
                    .update(cx, |view, cx| view.seed_status_click_page(cx));
                window.draw(cx).clear(cx);
                window.click("status-prs", cx);
                let app = shell.read(cx);
                assert_eq!(app.active_section, Some(Section::GitHub));
                assert_eq!(app.github_view.entity_id(), retained);
                app.github_view.read(cx).assert_status_click_reset();
            });
        }
    }

    fn assert_automation_status_click(cx: &mut gpui_kit::TestAppContext, target: ViewTarget) {
        use gpui_kit::test::TestWindowExt as _;
        let (window, shell) = status_click_shell(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        let retained = visual.update(|_, cx| shell.read(cx).automation_view.entity_id());
        for _ in 0..2 {
            visual.update(|window, cx| {
                shell
                    .read(cx)
                    .automation_view
                    .clone()
                    .update(cx, |view, cx| view.seed_status_click_page(target, cx));
                window.draw(cx).clear(cx);
                window.click(
                    if target == ViewTarget::AutomationSchedules {
                        "status-schedules"
                    } else {
                        "status-jobs"
                    },
                    cx,
                );
                let app = shell.read(cx);
                assert_eq!(app.active_section, Some(Section::Automation));
                assert_eq!(app.automation_view.entity_id(), retained);
                app.automation_view
                    .read(cx)
                    .assert_status_click_reset(target);
            });
        }
    }

    #[gpui_kit::test]
    fn status_schedules_click_selects_schedules_and_resets_pages(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        if status_click_child(
            "app::tests::status_schedules_click_selects_schedules_and_resets_pages",
        ) {
            return;
        }
        assert_automation_status_click(cx, ViewTarget::AutomationSchedules);
    }

    /// Electron's `N jobs` item opens `automation-runs`, the run history.
    #[gpui_kit::test]
    fn status_jobs_click_selects_run_history_and_resets_pages(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::status_jobs_click_selects_run_history_and_resets_pages")
        {
            return;
        }
        assert_automation_status_click(cx, ViewTarget::AutomationRuns);
    }

    /// `@account` appears only when `gh` has an active account, as
    /// `GitHubAccountItem` renders nothing otherwise.
    #[gpui_kit::test]
    fn status_account_item_shows_only_with_an_active_account(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::status_account_item_shows_only_with_an_active_account") {
            return;
        }
        use gpui_kit::test::TestWindowExt as _;
        let (window, shell) = status_click_shell(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.try_find("status-account").is_none());
            shell.update(cx, |app, cx| {
                app.active_account = Some("octo".into());
                cx.notify();
            });
            window.draw(cx).clear(cx);
            // Its click runs `open_target(SettingsAccounts)`, proven by
            // `view_targets_open_their_specific_views`. Clicking here would
            // render Settings, whose stored-credential check needs a Tokio
            // reactor that the deterministic test scheduler cannot host.
            assert!(window.try_find("status-account").is_some());
        });
    }

    /// The dashboard's Open Usage, Configure Accounts and quick actions open
    /// the specific list Electron names, not the section's retained page.
    #[gpui_kit::test]
    fn view_targets_open_their_specific_views(cx: &mut gpui_kit::TestAppContext) {
        if status_click_child("app::tests::view_targets_open_their_specific_views") {
            return;
        }
        let (window, shell) = status_click_shell(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.update(|_, cx| {
            shell.update(cx, |app, cx| {
                app.settings_view
                    .update(cx, |view, cx| view.seed_other_panel(cx));
                app.open_target(ViewTarget::SettingsAccounts, cx);
                assert_eq!(app.active_section, Some(Section::Settings));
                assert!(app.settings_view.read(cx).showing_accounts());

                assert!(!app.copilot_view.read(cx).showing_usage());
                app.open_target(ViewTarget::CopilotUsage, cx);
                assert_eq!(app.active_section, Some(Section::Copilot));
                assert!(app.copilot_view.read(cx).showing_usage());

                app.open_target(ViewTarget::AutomationJobs, cx);
                assert_eq!(app.active_section, Some(Section::Automation));
                app.automation_view
                    .read(cx)
                    .assert_status_click_reset(ViewTarget::AutomationJobs);

                app.open_target(ViewTarget::Section(Section::GitHub), cx);
                assert_eq!(app.active_section, Some(Section::GitHub));
            });
        });
    }

    struct HistoryShortcutProbe {
        focus: gpui_kit::FocusHandle,
        context: &'static str,
        history: usize,
        terminal_keys: usize,
    }
    impl Render for HistoryShortcutProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context("BuddyApp")
                .on_action(cx.listener(|this, _: &NavigationBack, _, _| this.history += 1))
                .on_action(cx.listener(|this, _: &NavigationForward, _, _| this.history += 1))
                .child(
                    div().key_context(self.context).child(
                        div()
                            .key_context("Input")
                            .track_focus(&self.focus)
                            .on_key_down(cx.listener(
                                |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                                    if this.context.starts_with("BuddyTerminal")
                                        && event.keystroke.modifiers.alt
                                        && ["left", "right"].contains(&event.keystroke.key.as_str())
                                        && !crate::browser_view::keys::claimed(cx)
                                            .contains(&event.keystroke)
                                    {
                                        this.terminal_keys += 1;
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                    ),
                )
        }
    }

    #[gpui_kit::test]
    fn history_shortcuts_leave_alt_arrows_with_terminal_and_paste_contexts(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (window, view) = cx.update(|cx| {
            init_navigation(cx);
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    HistoryShortcutProbe {
                        focus,
                        context: "Workspace",
                        history: 0,
                        terminal_keys: 0,
                    }
                })
            })
            .unwrap()
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("alt-left alt-right");
        view.read_with(&visual, |view, _| assert_eq!(view.history, 2));
        for context in ["BuddyTerminal", "BuddyTerminalPaste"] {
            view.update(&mut visual, |view, cx| {
                view.context = context;
                cx.notify();
            });
            visual.run_until_parked();
            visual.simulate_keystrokes("alt-left alt-right");
            view.read_with(&visual, |view, _| assert_eq!(view.history, 2, "{context}"));
        }
        view.read_with(&visual, |view, _| assert_eq!(view.terminal_keys, 4));
    }

    #[test]
    fn every_section_id_maps_back_to_its_section() {
        for section in Section::ALL {
            assert_eq!(Section::from_id(section.id()), Some(section));
        }
        assert_eq!(Section::from_id("dashboard"), None);
        assert_eq!(Section::from_id("retired-section"), None);
    }

    #[gpui_kit::test]
    fn reloaded_section_updates_the_shell_without_starting_hidden_dashboard_io(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let shell = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut config = buddy_core::config::AppConfig::default();
            config.native.active_section = Some("github".into());
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            let mut entity = None;
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                let dashboard = cx.new(|cx| DashboardView::new(window, cx, false));
                let shell = cx.new(|cx| {
                    BuddyApp::observe_settings(window, cx);
                    BuddyApp {
                        active_section: Some(Section::GitHub),
                        active_account: None,
                        dashboard,
                        settings_view: cx.new(|cx| SettingsView::new(window, cx)),
                        github_view: cx.new(|cx| GitHubView::new(window, cx)),
                        terminal_view: cx.new(|cx| TerminalView::new(window, cx)),
                        terminal_panel: cx
                            .new(|cx| crate::terminal_view::panel::TerminalPanel::new(window, cx)),
                        copilot_view: cx.new(|cx| CopilotView::new(window, cx)),
                        assistant: cx
                            .new(|cx| crate::copilot_view::chat::ChatView::new(window, cx)),
                        todoist_view: cx.new(|cx| TodoistView::new(window, cx)),
                        tempo_view: cx.new(|cx| TempoView::new(window, cx)),
                        automation_view: cx.new(|cx| AutomationView::new(window, cx)),
                        ralph_view: cx.new(|cx| RalphView::new(window, cx)),
                        crew_view: cx.new(|cx| CrewView::new(window, cx)),
                        bookmarks_view: cx.new(|cx| BookmarksView::new(window, cx)),
                        browser_view: cx.new(|cx| BrowserView::new(window, cx)),
                        explorer_view: cx.new(|_| crate::explorer_view::ExplorerView::new()),
                        folder_open: false,
                        window_state: WindowPersistence::new(),
                        focus: BuddyApp::initial_focus(window, cx),
                        tabs: crate::shell::navigation::Tabs::default(),
                        sidebar_split: SidebarSplit::new(cx),
                    }
                });
                entity = Some(shell.clone());
                shell
            })
            .unwrap();
            entity.unwrap()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.native.active_section = Some("terminal".into());
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(shell.read(cx).active_section, Some(Section::Terminal)));
        let assistant_id = cx.update(|cx| shell.read(cx).assistant.entity_id());
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.ui.assistant_open = true;
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            use gpui_kit::test::TestWindowExt as _;
            let handle = cx.windows()[0];
            handle
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    // Buttons are observed by GPUI Kit; plain layout containers
                    // provide identity scopes but do not register their own bounds.
                    let suggestion = window
                        .within("assistant-panel")
                        .find(("assistant-suggestion", 0usize));
                    assert!(suggestion.visible());
                    let width = f32::from(suggestion.bounds().size.width) + 24.0;
                    assert!((280.0..=600.0).contains(&width), "Assistant width: {width}");
                })
                .unwrap();
            assert_eq!(shell.read(cx).assistant.entity_id(), assistant_id);
            assert_eq!(shell.read(cx).active_section, Some(Section::Terminal));
        });
        let panel_id = cx.update(|cx| shell.read(cx).terminal_panel.entity_id());
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.ui.terminal_open = true;
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            use gpui_kit::test::TestWindowExt as _;
            cx.windows()[0]
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find("panel-new").visible());
                    assert!(window.find("panel-hide").visible());
                })
                .unwrap();
            assert_eq!(shell.read(cx).terminal_panel.entity_id(), panel_id);
            assert_eq!(shell.read(cx).active_section, Some(Section::Terminal));
        });
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.ui.terminal_open = false;
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(shell.read(cx).terminal_panel.entity_id(), panel_id);
            assert_eq!(shell.read(cx).assistant.entity_id(), assistant_id);
        });
        // Exercise the actual shell actions without a config write: both
        // destinations belong to the already-selected Terminal section.
        cx.update(|cx| {
            init_navigation(cx);
            shell.update(cx, |shell, cx| {
                shell.tabs = crate::shell::navigation::Tabs::default();
                for name in ["terminal-a", "terminal-b"] {
                    let mut d =
                        crate::shell::navigation::Destination::section(Some(Section::Terminal));
                    d.key = name.into();
                    shell.tabs.open(d);
                }
                cx.notify();
            });
            cx.windows()[0]
                .update(cx, |_, window, cx| {
                    let focus = shell.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    window.dispatch_action(Box::new(NextGlobalTab), cx);
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("terminal-a")));
        cx.update(|cx| {
            cx.windows()[0]
                .update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(CloseGlobalTab), cx)
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(shell.read(cx).tabs.active.as_deref(), Some("terminal-b"));
            assert_eq!(shell.read(cx).active_section, Some(Section::Terminal));
            assert_eq!(shell.read(cx).terminal_panel.entity_id(), panel_id);
        });
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.native.active_section = Some("automation".into());
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            use gpui_kit::test::TestWindowExt as _;
            cx.windows()[0]
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    let refresh = window.find("automation-refresh");
                    assert!(refresh.visible());
                    assert!(f32::from(refresh.bounds().size.height) >= 20.0);
                })
                .unwrap();
        });
        // Settings notifications remove unavailable destinations immediately,
        // including inactive tabs, without waiting for the user to select one.
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.github.accounts = serde_json::from_value(serde_json::json!([
                {"username":"First", "org":"team"}
            ]))
            .unwrap();
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        let removed_key = cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                let destination = shell.destination(Some(Section::GitHub), cx);
                let key = destination.key.clone();
                shell.tabs.open(destination);
                shell.tabs.select("terminal-b");
                key
            })
        });
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.github.accounts.clear();
            Settings::replace(cx, config);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                shell
                    .read(cx)
                    .tabs
                    .items
                    .iter()
                    .all(|tab| tab.key != removed_key)
            );
            assert!(
                shell.read(cx).tabs.closed.is_empty(),
                "pruned destinations must complete the close path"
            );
        });
    }
}
