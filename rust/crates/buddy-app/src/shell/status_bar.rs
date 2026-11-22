//! Port of `StatusBar.tsx`: compact status items on the configured colors.
//! Only metrics the native app can actually provide are shown.

use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::{Context, IntoElement, ParentElement, SharedString, Styled};

use crate::app::{BuddyApp, ViewTarget};
use crate::dashboard::SyncStatus;
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

/// A status item that only explains itself on hover (`data-tooltip` on a
/// plain `.status-item`). It is not a tab stop because it does nothing.
fn info_item(
    id: &'static str,
    icon: IconName,
    label: impl Into<SharedString>,
    tooltip: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .tab_stop(false)
        .icon(icon)
        .label(label)
        .tooltip(tooltip)
}

/// A status item that opens `target` when clicked (`status-item-clickable`).
fn link_item(
    id: &'static str,
    label: String,
    tooltip: &'static str,
    target: ViewTarget,
    cx: &mut Context<BuddyApp>,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .label(label)
        .tooltip(tooltip)
        .on_click(cx.listener(move |this, _, _, cx| this.open_target(target, cx)))
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
    // merely the first configured account. Hidden when there is none.
    let account = app.active_account.clone();
    // `formatTime(now, { seconds: true })`: the locale's clock convention.
    let now = jiff::Zoned::now();
    let clock = buddy_core::format::clock(now.time())
        .unwrap_or_else(|| now.strftime("%-I:%M:%S %p").to_string());
    let date = buddy_core::format::weekday_date(now.date())
        .unwrap_or_else(|| now.strftime("%a, %b %-d").to_string());
    let settings = Settings::global(cx);
    let save_error = settings.save_error.clone();
    let assistant_open = settings.config.ui.assistant_open;
    let dashboard = app.dashboard.read(cx);
    let (prs, pr_stale) = dashboard.status_prs();
    let sync = dashboard.sync_status(cx);
    let (jobs, schedules, automation_stale) = app.automation_view.read(cx).status_totals();

    StatusBar::new()
        .h(zpx(STATUS_BAR_HEIGHT))
        .bg(theme.status_bar)
        .border_t_1()
        .border_color(theme.status_bar_border)
        .text_color(palette.status_bar_fg)
        .text_size(zpx(11.0))
        .left(link_item(
            "status-prs",
            count_label(prs, "PRs", pr_stale),
            "View Pull Requests. Total sums the four PR lists, as on the dashboard.",
            ViewTarget::MyPrs,
            cx,
        ))
        .left(link_item(
            "status-schedules",
            count_label(schedules, "schedules", automation_stale),
            "View Schedules",
            ViewTarget::AutomationSchedules,
            cx,
        ))
        // `automation-runs`: the jobs total opens the run history.
        .left(link_item(
            "status-jobs",
            count_label(jobs, "jobs", automation_stale),
            "View Jobs",
            ViewTarget::AutomationRuns,
            cx,
        ))
        .left(gpui_kit::div().children(account.map(|account| {
            link_item(
                "status-account",
                format!("@{account}"),
                "View Account Settings",
                ViewTarget::SettingsAccounts,
                cx,
            )
            .icon(IconName::User)
        })))
        .left(gpui_kit::div().children(sync.label().map(|label| {
            let icon = if sync == SyncStatus::Syncing {
                IconName::RefreshCw
            } else {
                IconName::CircleCheck
            };
            info_item("status-sync", icon, label, sync.tooltip())
        })))
        .left(gpui_kit::div().children(
            save_error.map(|error| {
                status_item(IconName::Settings, format!("Settings not saved: {error}"))
            }),
        ))
        // `.status-bar-center`: the assistant indicator, then the brand.
        .child(
            h_flex()
                .h_full()
                .items_center()
                .children(assistant_open.then(|| {
                    info_item(
                        "status-copilot",
                        IconName::Sparkles,
                        "Copilot",
                        "Copilot Assistant active",
                    )
                }))
                .child(info_item(
                    "status-brand",
                    IconName::Bot,
                    "Buddy",
                    "hs-buddy",
                )),
        )
        .right(info_item(
            "status-date",
            IconName::Calendar,
            date,
            "Current Date",
        ))
        .right(info_item(
            "status-clock",
            IconName::Clock,
            clock,
            "Current Time",
        ))
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
