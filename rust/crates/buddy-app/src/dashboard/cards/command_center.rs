//! `CommandCenterCard`: Copilot usage summary (span 2).

use buddy_core::dashboard::CardId;
use buddy_core::format::{currency, thousands};
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, Styled, div, linear_color_stop,
    linear_gradient,
};

use crate::app::ViewTarget;
use crate::dashboard::DashboardView;
use crate::dashboard::primitives::{Pill, StatCard, icon_box, section, section_heading, stat_grid};
use crate::theme::{BuddyPalette, hex};
use crate::zoom::zpx;

pub fn render(view: &DashboardView, cx: &mut Context<DashboardView>) -> AnyElement {
    let summary = view.command_center().clone();
    let copilot_error = view.copilot_error();
    let palette = *BuddyPalette::global(cx);
    let secondary = cx.theme().secondary;
    let has_accounts = summary.has_accounts();

    let accent = hex("#70e091");
    let copilot = hex("#5fe1ff");
    let copilot_soft = hex("#6ad6c0");
    let copilot_soft_bg = hex("#4ec9b0").opacity(0.12);
    let overage = hex("#f2b05e");
    let overage_bg = overage.opacity(0.14);
    let overage_card_border = hex("#e89b3c").opacity(0.22);
    let overage_card_bg = linear_gradient(
        135.0,
        linear_color_stop(hex("#e89b3c").opacity(0.08), 0.0),
        linear_color_stop(secondary, 1.0),
    );

    let projected = summary
        .projected_total
        .map(thousands)
        .unwrap_or_else(|| "…".to_string());
    let projected_overage = match summary.projected_overage_cost {
        Some(cost) if cost > 0.0 => currency(cost),
        _ => currency(0.0),
    };

    let open_label = if has_accounts {
        "Open Usage"
    } else {
        "Configure Accounts"
    };
    let open_target = if has_accounts {
        ViewTarget::CopilotUsage
    } else {
        ViewTarget::SettingsAccounts
    };

    let narrow = view.narrow();
    // `.welcome-usage-strip-header`: accounts beside the actions, or stacked
    // above full-width actions when narrow.
    let strip = if narrow {
        v_flex().items_start()
    } else {
        h_flex().items_center().justify_between().flex_wrap()
    };
    section(Some(accent), narrow, cx)
        .child(section_heading(
            "Copilot usage",
            "Command Center",
            "Live spend, projection, and account health at a glance",
            narrow,
            cx,
        ))
        .child(
            strip
                .w_full()
                .gap(zpx(16.0))
                .child(
                    h_flex()
                        .items_center()
                        .gap(zpx(10.0))
                        .child(icon_box(
                            32.0,
                            7.0,
                            IconName::Sparkles,
                            18.0,
                            copilot,
                            copilot.opacity(0.12),
                        ))
                        .child(
                            v_flex()
                                .gap(zpx(2.0))
                                .child(
                                    div()
                                        .text_size(zpx(12.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(palette.text_heading)
                                        .child("CONNECTED ACCOUNTS"),
                                )
                                .child(
                                    div()
                                        .text_size(zpx(11.0))
                                        .text_color(palette.text_secondary)
                                        .child(if summary.loading {
                                            "Refreshing usage…".to_string()
                                        } else {
                                            summary.account_description()
                                        }),
                                )
                                .when_some(copilot_error, |this, error| {
                                    this.child(
                                        div()
                                            .text_size(zpx(10.0))
                                            .text_color(palette.accent_error)
                                            .child(error),
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(zpx(8.0))
                        .when(narrow, |this| this.w_full().justify_between())
                        .child(
                            Pill::new("cc-refresh", "Refresh")
                                .icon(IconName::RefreshCw)
                                .disabled(summary.loading || !has_accounts)
                                .tooltip("Refresh Copilot usage data")
                                .build(
                                    cx.listener(|this, _, _, cx| this.refresh_copilot_usage(cx)),
                                    cx,
                                ),
                        )
                        .child(
                            Pill::new("cc-open", open_label)
                                .trailing(IconName::ArrowRight)
                                .build(
                                    cx.listener(move |this, _, _, cx| {
                                        this.navigate(open_target, cx)
                                    }),
                                    cx,
                                ),
                        ),
                ),
        )
        .child(stat_grid(
            vec![
                StatCard::new(IconName::Zap, thousands(summary.total_used), "Total Used")
                    .icon_colors(copilot_soft, copilot_soft_bg),
                StatCard::new(
                    IconName::Sparkles,
                    currency(summary.total_overage_cost),
                    "Total Overage",
                )
                .icon_colors(copilot_soft, copilot_soft_bg),
                StatCard::new(IconName::Activity, projected, "Projected")
                    .icon_colors(copilot_soft, copilot_soft_bg),
                StatCard::new(IconName::ArrowRight, projected_overage, "Est. Overage")
                    .icon_colors(overage, overage_bg)
                    .card_style(overage_card_bg, overage_card_border),
            ],
            // Wide enough for an amount such as "$17,457.30" beside the icon.
            view.stat_columns(CardId::CommandCenter, 180.0),
            cx,
        ))
        .into_any_element()
}
