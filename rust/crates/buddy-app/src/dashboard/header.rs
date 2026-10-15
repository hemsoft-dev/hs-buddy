//! `WelcomeHeader`: app identity, version, Customize popover, live uptime.

use buddy_core::dashboard::DASHBOARD_CARDS;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    BoxShadow, Context, FontWeight, IntoElement, ParentElement, Styled, div, linear_color_stop,
    linear_gradient, point,
};

use super::DashboardView;
use crate::settings::Settings;
use crate::theme::BuddyPalette;
use crate::zoom::{scaled, zpx};

/// The product version from `package.json` (see `build.rs`).
pub const APP_VERSION: &str = env!("BUDDY_VERSION");

fn badge(
    text: String,
    icon: Option<IconName>,
    elevated: bool,
    cx: &gpui_kit::App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let palette = BuddyPalette::global(cx);
    h_flex()
        .items_center()
        .gap(zpx(5.0))
        .px(zpx(10.0))
        .py(zpx(3.0))
        .rounded(zpx(20.0))
        .border_1()
        .border_color(theme.border)
        .bg(if elevated {
            palette.bg_hover
        } else {
            theme.secondary
        })
        .text_size(zpx(if elevated { 13.0 } else { 11.0 }))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if elevated {
            theme.foreground
        } else {
            palette.text_secondary
        })
        .when_some(icon, |this, icon| {
            this.child(Icon::new(icon).size(zpx(12.0)).text_color(theme.primary))
        })
        .child(text)
}

fn customize(cx: &mut Context<DashboardView>) -> impl IntoElement + use<> {
    let weak = cx.entity().downgrade();
    let row_radius = scaled(cx, 6.0);
    Popover::new("dashboard-config")
        .trigger(
            Button::new("dashboard-config-trigger")
                .outline()
                .xsmall()
                .icon(Icon::new(IconName::Settings).size(zpx(14.0)))
                .label("Customize")
                .tooltip("Configure dashboard cards"),
        )
        .content(move |_, _, cx| {
            let palette = *BuddyPalette::global(cx);
            let primary = cx.theme().primary;
            let foreground = cx.theme().foreground;
            // Rows are real buttons: focusable, in the tab order, keyboard-toggled.
            let row = ButtonCustomVariant::new(cx)
                .color(cx.theme().transparent)
                .foreground(foreground)
                .hover(palette.bg_hover)
                .active(palette.bg_hover)
                .shadow(false);
            let visibility: Vec<bool> = DASHBOARD_CARDS
                .iter()
                .map(|card| {
                    Settings::global(cx)
                        .config
                        .is_dashboard_card_visible(card.key())
                })
                .collect();

            v_flex()
                .min_w(zpx(200.0))
                .child(
                    div()
                        .px(zpx(8.0))
                        .pt(zpx(6.0))
                        .pb(zpx(8.0))
                        .text_size(zpx(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text_muted)
                        .child("DASHBOARD CARDS"),
                )
                .children(
                    DASHBOARD_CARDS
                        .into_iter()
                        .zip(visibility)
                        .map(|(card, visible)| {
                            let weak = weak.clone();
                            let action = if visible { "Hide" } else { "Show" };
                            Button::new(card.key())
                                .custom(row)
                                .toggled(visible)
                                .accessibility_label(format!("{action} {}", card.title()))
                                .w_full()
                                .p(zpx(8.0))
                                .rounded(row_radius)
                                .font_weight(FontWeight::MEDIUM)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .items_center()
                                        .gap(zpx(8.0))
                                        .text_size(zpx(12.0))
                                        .child(if visible {
                                            Icon::new(IconName::Eye)
                                                .size(zpx(14.0))
                                                .text_color(primary)
                                        } else {
                                            Icon::new(IconName::EyeOff)
                                                .size(zpx(14.0))
                                                .text_color(palette.text_muted.opacity(0.6))
                                        })
                                        .child(card.title()),
                                )
                                .on_click(move |_, _, cx| {
                                    weak.update(cx, |this, cx| this.toggle_card(card, cx)).ok();
                                })
                        }),
                )
        })
}

pub fn render(view: &DashboardView, cx: &mut Context<DashboardView>) -> impl IntoElement + use<> {
    let palette = *BuddyPalette::global(cx);
    let background = cx.theme().background;
    let uptime_ms = view.live_uptime_ms();
    let narrow = view.narrow();
    // `max-width: 680px`: the row wraps, the meta column becomes a row and
    // its top row (version, Customize) a right-aligned column.
    let meta = if narrow {
        h_flex().items_center().gap(zpx(6.0))
    } else {
        v_flex().items_end().gap(zpx(6.0))
    };
    let meta_top = if narrow {
        v_flex().items_end().gap(zpx(4.0))
    } else {
        h_flex().items_center().gap(zpx(8.0))
    };

    h_flex()
        .w_full()
        .items_center()
        .gap(zpx(16.0))
        .when(narrow, |this| this.flex_wrap())
        .child(
            div()
                .size(zpx(52.0))
                .flex_shrink_0()
                .rounded(zpx(12.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(linear_gradient(
                    135.0,
                    linear_color_stop(palette.gold, 0.0),
                    linear_color_stop(palette.orange, 0.5),
                ))
                .shadow(vec![
                    BoxShadow {
                        color: palette.gold.opacity(0.25),
                        offset: point(scaled(cx, 0.0), scaled(cx, 4.0)),
                        blur_radius: scaled(cx, 16.0),
                        spread_radius: scaled(cx, 0.0),
                        inset: false,
                    },
                    BoxShadow {
                        color: gpui_kit::black().opacity(0.25),
                        offset: point(scaled(cx, 0.0), scaled(cx, 2.0)),
                        blur_radius: scaled(cx, 8.0),
                        spread_radius: scaled(cx, 0.0),
                        inset: false,
                    },
                ])
                .text_color(background)
                .child(Icon::new(IconName::Users).size(zpx(32.0))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                // Narrow: keep room for the subtitle and wrap the meta below.
                .when(narrow, |this| this.flex_basis(zpx(200.0)))
                .gap(zpx(2.0))
                .child(
                    div()
                        .text_size(zpx(24.0))
                        .line_height(zpx(29.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text_heading)
                        .child("Buddy"),
                )
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap(zpx(6.0))
                        .text_size(zpx(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.gold)
                        .child(Icon::new(IconName::Handshake).size(zpx(15.0)))
                        .child(
                            div()
                                .min_w_0()
                                .child("Your Universal Productivity Companion"),
                        ),
                ),
        )
        .child(
            meta.flex_shrink_0()
                .child(
                    meta_top
                        .child(badge(format!("Version {APP_VERSION}"), None, true, cx))
                        .child(customize(cx)),
                )
                .when(uptime_ms > 0, |this| {
                    this.child(badge(
                        buddy_core::format::uptime(uptime_ms),
                        Some(IconName::Clock),
                        false,
                        cx,
                    ))
                }),
        )
}
