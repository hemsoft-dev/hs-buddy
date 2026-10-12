//! Root view: the application shell around the active content.

use std::time::Duration;

use buddy_core::config::DisplayRect;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div,
};

use crate::dashboard::{DashboardEvent, DashboardView};
use crate::platform_placement;
use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::shell::{activity_bar, status_bar, tab_bar, title_bar};
use crate::window_persistence::{self, Placement, SAVE_DELAY, WindowPersistence};
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
    window_state: WindowPersistence,
}

impl BuddyApp {
    /// Show a section (`None` is the dashboard) and remember it for the next
    /// launch; the dashboard pauses its refreshes while hidden.
    pub fn set_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
        if self.active_section != section {
            Settings::update(cx, |config| {
                config.native.active_section = section.map(|section| section.id().to_string())
            });
        }
        self.show_section(section, cx);
    }

    fn show_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
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
        let outer = platform_placement::restore_bounds(window)?;
        let bounds = match reported {
            gpui_kit::WindowBounds::Windowed(_) => gpui_kit::WindowBounds::Windowed(outer),
            gpui_kit::WindowBounds::Maximized(_) => gpui_kit::WindowBounds::Maximized(outer),
            gpui_kit::WindowBounds::Fullscreen(_) => gpui_kit::WindowBounds::Fullscreen(outer),
        };
        let display = window.display(cx);
        Some(Placement {
            bounds,
            is_maximized: window.is_maximized(),
            is_full_screen: window.is_fullscreen(),
            native_display_uuid: display
                .as_ref()
                .and_then(|display| display.uuid().ok())
                .map(|id| id.to_string()),
            display: display.map(|display| display.bounds()),
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

    /// Electron moves its window back onto the display in `ui.displayId`, so
    /// once the saved native window sits on another display, clear that id;
    /// Electron then restores `window-state.json`, which it still checks
    /// against the connected displays. Call only after that file holds the
    /// latest placement. Electron's ids are not GPUI's, so the displays are
    /// compared by their bounds.
    fn release_electron_display(&self, cx: &mut App) {
        let ui = &Settings::global(cx).config.ui;
        let current = self
            .window_state
            .latest()
            .and_then(|state| state.display_bounds);
        let moved = current.is_some_and(|current| {
            window_persistence::different_displays(ui.display_bounds, current)
        });
        if ui.display_id == 0 || !moved {
            return;
        }
        Settings::update(cx, |config| {
            config.ui.display_id = 0;
            config.ui.display_bounds = DisplayRect::default();
            config.ui.display_work_area = DisplayRect::default();
        });
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dashboard = cx.new(|cx| DashboardView::new(window, cx));
        cx.subscribe(&dashboard, |this, _, event, cx| {
            let DashboardEvent::Navigate(section) = event;
            this.set_section(Some(*section), cx);
        })
        .detach();

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

        let mut this = Self {
            active_section: None,
            active_account: None,
            dashboard,
            window_state: WindowPersistence::new(),
        };
        let saved = Settings::global(cx).config.native.active_section.clone();
        let section = saved.as_deref().and_then(Section::from_id);
        if section.is_some() {
            this.show_section(section, cx);
        }
        this.window_bounds_changed(window, cx);
        this
    }

    fn render_content(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let container = div().flex_1().min_h_0().w_full().bg(theme.background);
        match self.active_section {
            None => container.child(self.dashboard.clone()),
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
}
