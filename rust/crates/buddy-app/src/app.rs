//! Root view: the application shell around the active content.

use std::time::Duration;

use buddy_core::config::DisplayRect;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div,
};

use crate::copilot_view::CopilotView;
use crate::dashboard::{DashboardEvent, DashboardView};
use crate::github_view::GitHubView;
use crate::platform_placement;
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
        ResetZoom
    ]
);

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
    dashboard: Entity<DashboardView>,
    settings_view: Entity<SettingsView>,
    github_view: Entity<GitHubView>,
    terminal_view: Entity<TerminalView>,
    copilot_view: Entity<CopilotView>,
    todoist_view: Entity<TodoistView>,
    tempo_view: Entity<TempoView>,
    window_state: WindowPersistence,
    focus: FocusHandle,
}

impl BuddyApp {
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
        self.show_section(section, cx);
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
        if section == Some(Section::Settings) && self.active_section != section {
            self.settings_view
                .update(cx, |settings, cx| settings.activate(cx));
        }
        if section != Some(Section::Settings) {
            self.settings_view
                .update(cx, |settings, _| settings.deactivate());
        }
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

    fn observe_settings(cx: &mut Context<Self>) {
        cx.observe_global::<Settings>(|this, cx| {
            let saved = &Settings::global(cx).config.native.active_section;
            let section = saved.as_deref().and_then(Section::from_id);
            if this.active_section != section {
                this.show_section(section, cx);
            }
            cx.notify();
        })
        .detach();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = Settings::global(cx).config.native.active_section.clone();
        let section = saved.as_deref().and_then(Section::from_id);
        let dashboard = cx.new(|cx| DashboardView::new(window, cx, section.is_none()));
        cx.subscribe(&dashboard, |this, _, event, cx| {
            let DashboardEvent::Navigate(section) = event;
            this.set_section(Some(*section), cx);
        })
        .detach();

        Self::observe_settings(cx);

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
        let terminal_view = cx.new(|cx| TerminalView::new(window, cx));
        let copilot_view = cx.new(|cx| CopilotView::new(window, cx));
        let todoist_view = cx.new(|cx| TodoistView::new(window, cx));
        let tempo_view = cx.new(|cx| TempoView::new(window, cx));
        if section == Some(Section::Tempo) {
            tempo_view.update(cx, |view, cx| view.activate(cx));
        }
        if section == Some(Section::Tasks) {
            todoist_view.update(cx, |view, cx| view.activate(cx));
        }
        cx.subscribe(
            &github_view,
            |this, _, event: &crate::github_view::GitHubEvent, cx| {
                let crate::github_view::GitHubEvent::PrepareAssistantReview(client, pull) = event;
                this.set_section(Some(Section::Copilot), cx);
                this.copilot_view.update(cx, |view, cx| {
                    view.prepare_review(client.clone(), pull.clone(), cx)
                });
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
        cx.subscribe(&settings_view, |this, _, event, cx| match event {
            SettingsEvent::ClearCache => {
                this.dashboard
                    .update(cx, |dashboard, cx| dashboard.clear_cached_data(cx));
                this.github_view.update(cx, |view, cx| view.clear_cache(cx));
            }
        })
        .detach();
        let mut this = Self {
            active_section: section,
            active_account: None,
            dashboard,
            settings_view,
            github_view,
            terminal_view,
            copilot_view,
            todoist_view,
            tempo_view,
            window_state: WindowPersistence::new(),
            focus: Self::initial_focus(window, cx),
        };
        this.window_bounds_changed(window, cx);
        this
    }

    fn render_content(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let container = div().flex_1().min_h_0().w_full().bg(theme.background);
        match self.active_section {
            None => container.child(self.dashboard.clone()),
            Some(Section::Settings) => container.child(self.settings_view.clone()),
            Some(Section::GitHub) => container.child(self.github_view.clone()),
            Some(Section::Terminal) => container.child(self.terminal_view.clone()),
            Some(Section::Copilot) => container.child(self.copilot_view.clone()),
            Some(Section::Tasks) => container.child(self.todoist_view.clone()),
            Some(Section::Tempo) => container.child(self.tempo_view.clone()),
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
        let theme = cx.theme();
        let font_family = theme.font_family.clone();
        let background = theme.background;
        let foreground = theme.foreground;

        v_flex()
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
                            .child(tab_bar::render(cx))
                            .child(self.render_content(cx)),
                    ),
            )
            .child(status_bar::render(self, cx))
            .child(zoom::wheel_listener())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    BuddyApp::observe_settings(cx);
                    BuddyApp {
                        active_section: Some(Section::GitHub),
                        active_account: None,
                        dashboard,
                        settings_view: cx.new(|cx| SettingsView::new(window, cx)),
                        github_view: cx.new(|cx| GitHubView::new(window, cx)),
                        terminal_view: cx.new(|cx| TerminalView::new(window, cx)),
                        copilot_view: cx.new(|cx| CopilotView::new(window, cx)),
                        todoist_view: cx.new(|cx| TodoistView::new(window, cx)),
                        tempo_view: cx.new(|cx| TempoView::new(window, cx)),
                        window_state: WindowPersistence::new(),
                        focus: BuddyApp::initial_focus(window, cx),
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
    }
}
