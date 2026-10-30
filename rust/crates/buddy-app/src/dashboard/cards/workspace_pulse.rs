//! `WorkspacePulseCard`: lifetime activity counters.

use buddy_core::dashboard::CardId;
use buddy_core::format::{month_year, thousands};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, div};

use crate::dashboard::DashboardView;
use crate::dashboard::primitives::{Pill, StatCard, section, section_heading, stat_grid};
use crate::theme::{BuddyPalette, hex};
use crate::zoom::zpx;

fn member_since(first_launch_ms: u64) -> String {
    if first_launch_ms == 0 {
        return "Today".to_string();
    }
    // `toLocaleDateString` formats in the host time zone, so convert first.
    chrono::DateTime::from_timestamp_millis(first_launch_ms as i64)
        .and_then(|date| month_year(date.with_timezone(&chrono::Local).date_naive()))
        .unwrap_or_else(|| "Today".to_string())
}

pub fn render(view: &DashboardView, cx: &mut Context<DashboardView>) -> AnyElement {
    let pulse = view.pulse();
    let palette = *BuddyPalette::global(cx);
    let status = view
        .convex_error()
        .or_else(|| (!view.stats_loaded()).then(|| "Connecting to Convex…".to_string()));
    let count = |n: u64| thousands(n as i64);

    let mut cards = vec![
        StatCard::new(
            IconName::GitPullRequest,
            count(pulse.total_prs_viewed),
            "PRs Viewed",
        ),
        StatCard::new(
            IconName::Sparkles,
            count(pulse.copilot_pr_reviews),
            "PRs Reviewed",
        ),
        StatCard::new(
            IconName::FolderGit2,
            count(pulse.repos_browsed),
            "Repos Browsed",
        ),
        StatCard::new(IconName::Play, count(pulse.runs_triggered), "Runs Executed")
            .subtitle((pulse.total_finished > 0).then(|| format!("{}%", pulse.success_rate))),
        StatCard::new(IconName::Star, count(pulse.bookmarks), "Bookmarks"),
        StatCard::new(
            IconName::Calendar,
            member_since(pulse.first_launch_ms),
            "Member Since",
        )
        .subtitle((pulse.app_launches > 0).then(|| {
            let plural = if pulse.app_launches == 1 { "" } else { "s" };
            format!("{} session{plural}", count(pulse.app_launches))
        })),
    ];
    {
        cards.insert(
            1,
            StatCard::new(
                IconName::Activity,
                pulse.active_prs.map(count).unwrap_or_else(|| "...".into()),
                "Active PRs",
            )
            .icon_colors(palette.accent_success, hex("#4ec9b0").opacity(0.12)),
        );
    }

    // Wide enough for the longest label ("REVIEWED") beside the icon.
    let columns = view.stat_columns(CardId::WorkspacePulse, 128.0);

    section(Some(hex("#50aaff")), view.narrow(), cx)
        .child(section_heading(
            "Buddy activity",
            "Workspace Pulse",
            "Pull requests, runs, bookmarks, and session history in one panel",
            view.narrow(),
            cx,
        ))
        .when_some(status, |this, status| {
            this.child(
                div()
                    .text_size(zpx(11.0))
                    .text_color(palette.text_muted)
                    .child(status),
            )
        })
        .child(
            Pill::new("pulse-refresh-prs", "Refresh PR totals")
                .icon(IconName::RefreshCw)
                .disabled(view.active_prs.loading)
                .build(
                    cx.listener(|this, _, _, cx| this.refresh_active_prs(true, cx)),
                    cx,
                ),
        )
        .when_some(view.active_prs.error.clone(), |this, error| {
            this.child(
                div()
                    .text_size(zpx(11.0))
                    .text_color(palette.text_muted)
                    .child(format!("PR totals: {error}")),
            )
        })
        .when(view.active_prs.loading, |this| {
            this.child(
                div()
                    .text_size(zpx(11.0))
                    .text_color(palette.text_muted)
                    .child("Loading PR totals..."),
            )
        })
        .child(stat_grid(cards, columns, cx))
        .into_any_element()
}
