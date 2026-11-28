//! The Insights section and the page a view with no route shows.
//!
//! Electron's Insights sidebar (`SidebarPanel.tsx`) lists Productivity and
//! Activity. Neither view id has a route, so `AppContentRouter.tsx` renders
//! its default case for each: a header naming the view (its
//! `appContentViewLabels.ts` label, else "Content") above "This feature is
//! coming soon!". This module ports that catch-all per view id and the two
//! sidebar entries that reach it.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestSupportExt as _, Window, div,
};

use crate::app::Section;
use crate::shell::navigation::{Destination, SidebarLink};
use crate::theme::BuddyPalette;
use crate::zoom::zpx;

/// The catch-all's text, as `AppContentRouter.tsx` renders it.
pub(crate) const COMING_SOON: &str = "This feature is coming soon!";

/// The heading for `view_id`: its `appContentViewLabels.ts` label, or
/// "Content" for an id that has none, as the router's default case shows.
pub(crate) fn view_label(view_id: &str) -> &'static str {
    match view_id {
        "dashboard" => "Dashboard",
        "pr-my-prs" => "My PRs",
        "pr-needs-review" => "Needs Review",
        "pr-recently-merged" => "Recently Merged",
        "pr-need-a-nudge" => "Needs a nudge",
        "terminal-workspace" => "Terminal Workspace",
        "tasks-today" => "Today",
        "tasks-upcoming" => "Upcoming",
        "tasks-projects" => "Projects",
        "insights-productivity" => "Productivity",
        "insights-activity" => "Activity",
        "settings-accounts" => "Accounts",
        "settings-appearance" => "Appearance",
        "settings-pullrequests" => "Pull Requests",
        "settings-copilot" => "Copilot SDK",
        "settings-notifications" => "Notifications",
        "settings-weather" => "Weather",
        "settings-advanced" => "Advanced",
        "automation-schedules" => "Schedules",
        "automation-runs" => "Runs",
        "crew-projects" => "The Crew",
        "copilot-prompt" => "Copilot Prompt",
        "copilot-all-results" => "Copilot Results",
        "copilot-usage" => "Copilot Usage",
        "copilot-sessions" => "Session Explorer",
        "bookmarks-all" => "All Bookmarks",
        "tempo-timesheet" => "Timesheet",
        "ralph-dashboard" => "Ralph Loops",
        _ => "Content",
    }
}

/// The router's default case for `view_id`: Electron's `.content-header`
/// (16px 24px padding, secondary background, bottom border, 20px heading)
/// over `.content-body` (24px padding, 16px text). Plain GPUI text has no
/// accessibility node, so both lines also carry their text as a name.
pub(crate) fn coming_soon(view_id: &str, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let palette = BuddyPalette::global(cx);
    let label = view_label(view_id);
    v_flex()
        .size_full()
        .text_color(theme.foreground)
        .child(
            div()
                .px(zpx(24.0))
                .py(zpx(16.0))
                .bg(theme.secondary)
                .border_b_1()
                .border_color(palette.border_secondary)
                .child(
                    div()
                        .id("coming-soon-heading")
                        .role(gpui_kit::Role::Heading)
                        .aria_label(label)
                        .test_support()
                        .text_size(zpx(20.0))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(label),
                ),
        )
        .child(
            div()
                .id("coming-soon-text")
                .role(gpui_kit::Role::Paragraph)
                .aria_label(COMING_SOON)
                .test_support()
                .flex_1()
                .p(zpx(24.0))
                .text_size(zpx(16.0))
                .child(COMING_SOON),
        )
}

/// The two views `SidebarPanel.tsx` lists under Insights, in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InsightsPage {
    #[default]
    Productivity,
    Activity,
}

impl InsightsPage {
    pub(crate) const ALL: [Self; 2] = [Self::Productivity, Self::Activity];

    /// Electron's view id, which keys the page's tab.
    pub(crate) fn view_id(self) -> &'static str {
        match self {
            Self::Productivity => "insights-productivity",
            Self::Activity => "insights-activity",
        }
    }
}

/// The Insights section: the page last opened, shown as the catch-all.
#[derive(Default)]
pub(crate) struct InsightsView {
    pub(crate) page: InsightsPage,
}

impl InsightsView {
    /// The tab for the page shown now, which the activity-bar entry opens.
    pub(crate) fn destination(&self, entity: Entity<Self>) -> Destination {
        Self::page_destination(self.page, entity)
    }

    fn page_destination(page: InsightsPage, entity: Entity<Self>) -> Destination {
        let mut destination = Destination::section(Some(Section::Insights));
        destination.key = page.view_id().into();
        destination.label = view_label(page.view_id()).into();
        destination.restore = Rc::new(move |_, cx| {
            entity.update(cx, |view, cx| {
                view.page = page;
                cx.notify();
            })
        });
        destination
    }

    /// Productivity and Activity. Each click is Electron's `openTab`, which
    /// counts one `tabsOpened`, an existing tab's included; a restored tab
    /// or a history step is not an open.
    pub(crate) fn sidebar(&self, entity: Entity<Self>) -> Vec<SidebarLink> {
        InsightsPage::ALL
            .into_iter()
            .map(|page| SidebarLink {
                destination: Self::page_destination(page, entity.clone()),
                on_open: Rc::new(|cx| crate::view_activity::record(cx)),
            })
            .collect()
    }
}

impl Render for InsightsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        coming_soon(self.page.view_id(), cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_electrons_view_labels_and_fall_back_to_content() {
        assert_eq!(view_label("insights-productivity"), "Productivity");
        assert_eq!(view_label("insights-activity"), "Activity");
        // Electron's Jobs quick action opens this id, which has no label.
        assert_eq!(view_label("automation-jobs"), "Content");
        assert_eq!(view_label(""), "Content");
        assert_eq!(
            InsightsPage::ALL.map(InsightsPage::view_id),
            ["insights-productivity", "insights-activity"]
        );
    }

    /// A view with no route of its own.
    struct Unrouted(&'static str);

    impl Render for Unrouted {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            coming_soon(self.0, cx)
        }
    }

    /// The catch-all is per view id: an id with no label still gets the
    /// router's "Content" heading over the coming-soon text.
    #[gpui_kit::test]
    fn a_view_without_a_route_shows_the_catch_all_for_its_id(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::test::TestWindowExt as _;
        for (view_id, heading) in [
            ("automation-jobs", "Content"),
            ("insights-activity", "Activity"),
        ] {
            let window: gpui_kit::AnyWindowHandle = cx
                .update(|cx| {
                    gpui_kit::init(cx);
                    crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
                    gpui_kit::open_window(Default::default(), cx, |_, cx| {
                        cx.new(|_| Unrouted(view_id))
                    })
                })
                .unwrap()
                .into();
            cx.update_window(window, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(window.find("coming-soon-heading").label(), Some(heading));
                assert_eq!(window.find("coming-soon-text").label(), Some(COMING_SOON));
            })
            .unwrap();
        }
    }
}
