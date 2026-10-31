//! Root view: the application shell around the active content.

use std::time::Duration;

use buddy_core::config::DisplayRect;
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
        gpui_kit::KeyBinding::new("secondary-tab", NextGlobalTab, Some("BuddyApp")),
        gpui_kit::KeyBinding::new("secondary-shift-tab", PreviousGlobalTab, Some("BuddyApp")),
        gpui_kit::KeyBinding::new("secondary-f4", CloseGlobalTab, Some("BuddyApp")),
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
        Settings::update(cx, move |config| config.ui.assistant_open = open);
        cx.refresh_windows();
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
            let DashboardEvent::Navigate(section) = event;
            this.set_section(Some(*section), cx);
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

    fn show_folder(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
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

    fn render_navigation(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::{
            Selectable as _,
            button::{Button, ButtonVariants as _},
        };
        let links = match self.active_section {
            Some(Section::GitHub) => self
                .github_view
                .read(cx)
                .sidebar(self.github_view.clone(), cx),
            Some(Section::Automation) => self
                .automation_view
                .read(cx)
                .sidebar(self.automation_view.clone()),
            _ => Vec::new(),
        };
        if links.is_empty() || self.folder_open {
            return div().into_any_element();
        }
        v_flex()
            .w(zpx(210.0))
            .h_full()
            .flex_shrink_0()
            .p_2()
            .gap_1()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(links.into_iter().enumerate().map(|(i, d)| {
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
                        this.tabs.open(d.clone());
                        this.restore_tab(window, cx);
                    }))
            }))
            .into_any_element()
    }

    fn render_workspace(&self, cx: &App) -> gpui_kit::AnyElement {
        use gpui_kit::component::resizable::{h_resizable, resizable_panel};
        if !Settings::global(cx).config.ui.assistant_open {
            return self.render_terminal_split(cx);
        }
        let width = Settings::global(cx)
            .config
            .ui
            .pane_sizes
            .get(2)
            .copied()
            .filter(|width| width.is_finite() && *width > 0.0)
            .unwrap_or(350.0)
            .clamp(280.0, 600.0);
        h_resizable("workspace-assistant-split")
            .child(
                resizable_panel()
                    .size_range(zoom::scaled(cx, 250.0)..gpui_kit::Pixels::MAX)
                    .child(self.render_terminal_split(cx)),
            )
            .child(
                resizable_panel()
                    .size(zoom::scaled(cx, width as f32))
                    .fixed()
                    .size_range(zoom::scaled(cx, 280.0)..zoom::scaled(cx, 600.0))
                    .child(
                        div()
                            .id("assistant-panel")
                            .size_full()
                            .p_3()
                            .child(self.assistant.clone()),
                    ),
            )
            .on_resize(|state, _, cx| {
                let Some(width) = state.read(cx).sizes().get(1).copied() else {
                    return;
                };
                let width = f64::from(f32::from(width) / f32::from(zoom::scaled(cx, 1.0)));
                Settings::update(cx, move |config| {
                    while config.ui.pane_sizes.len() < 3 {
                        config.ui.pane_sizes.push(350.0);
                    }
                    config.ui.pane_sizes[2] = width;
                });
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
        let theme = cx.theme();
        let font_family = theme.font_family.clone();
        let background = theme.background;
        let foreground = theme.foreground;

        v_flex()
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
                cx.listener(|this, _: &ToggleTerminal, window, cx| {
                    this.toggle_terminal(window, cx)
                }),
            )
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
                                div().flex().flex_col().flex_1().min_h_0().w_full().child(
                                    h_flex()
                                        .size_full()
                                        .child(self.render_navigation(cx))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .min_h_0()
                                                .h_full()
                                                .child(self.render_workspace(cx)),
                                        ),
                                ),
                            ),
                    ),
            )
            .child(status_bar::render(self, cx))
            .child(zoom::wheel_listener())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
