//! Port of `StatusBar.tsx`: compact status items on the configured colors.
//! Only metrics the native app can actually provide are shown.

use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled};

use crate::app::BuddyApp;
use crate::settings::Settings;
use crate::theme::BuddyPalette;
use crate::zoom::zpx;

pub const STATUS_BAR_HEIGHT: f32 = 22.0;

fn status_item(icon: IconName, text: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .h_full()
        .px(zpx(8.0))
        .gap(zpx(5.0))
        .items_center()
        .child(Icon::new(icon).size(zpx(12.0)))
        .child(text.into())
}

fn count_label(total: Option<u64>, noun: &str, stale: bool) -> String {
    match total {
        Some(total) if stale => format!("{total} {noun} (stale)"),
        Some(total) => format!("{total} {noun}"),
        None if stale => format!("{noun} unavailable"),
        None => format!("{noun} loading"),
    }
}

pub fn render(app: &BuddyApp, cx: &mut Context<BuddyApp>) -> impl IntoElement + use<> {
    let palette = BuddyPalette::global(cx);
    let theme = cx.theme();
    // The account `gh` is actually using, like the Electron status bar; not
    // merely the first configured account.
    let account = app
        .active_account
        .clone()
        .unwrap_or_else(|| "No gh account".to_string());
    // `formatTime(now, { seconds: true })`: the locale's clock convention.
    let now = chrono::Local::now();
    let clock = buddy_core::format::clock(now.time())
        .unwrap_or_else(|| now.format("%-I:%M:%S %p").to_string());
    let save_error = Settings::global(cx).save_error.clone();
    let (prs, pr_stale) = app.dashboard.read(cx).status_prs();
    let (jobs, schedules, automation_stale) = app.automation_view.read(cx).status_totals();

    StatusBar::new()
        .h(zpx(STATUS_BAR_HEIGHT))
        .bg(theme.status_bar)
        .border_t_1()
        .border_color(theme.status_bar_border)
        .text_color(palette.status_bar_fg)
        .text_size(zpx(11.0))
        .left(
            Button::new("status-prs")
                .ghost()
                .xsmall()
                .label(count_label(prs, "PRs", pr_stale))
                .tooltip("View Pull Requests. Total sums the four PR lists, as on the dashboard.")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_section(Some(crate::app::Section::GitHub), cx);
                    this.github_view
                        .update(cx, |view, cx| view.show_status_prs(cx));
                })),
        )
        .left(
            Button::new("status-schedules")
                .ghost()
                .xsmall()
                .label(count_label(schedules, "schedules", automation_stale))
                .tooltip("View Schedules")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_section(Some(crate::app::Section::Automation), cx);
                    this.automation_view
                        .update(cx, |view, cx| view.show_status_list(true, cx));
                })),
        )
        .left(
            Button::new("status-jobs")
                .ghost()
                .xsmall()
                .label(count_label(jobs, "jobs", automation_stale))
                .tooltip("View Jobs")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_section(Some(crate::app::Section::Automation), cx);
                    this.automation_view
                        .update(cx, |view, cx| view.show_status_list(false, cx));
                })),
        )
        .left(status_item(IconName::Users, account))
        .left(gpui_kit::div().children(
            save_error.map(|error| {
                status_item(IconName::Settings, format!("Settings not saved: {error}"))
            }),
        ))
        .right(status_item(IconName::Clock, clock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_failed_totals_do_not_claim_zero() {
        assert_eq!(count_label(None, "PRs", false), "PRs loading");
        assert_eq!(count_label(None, "PRs", true), "PRs unavailable");
        assert_eq!(count_label(Some(0), "jobs", false), "0 jobs");
        assert_eq!(count_label(Some(7), "jobs", true), "7 jobs (stale)");
    }
}
