//! Port of `ActivityBar.tsx`: a 48px vertical strip of section icons.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Icon, Selectable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, div,
};

use crate::app::{BuddyApp, Section};
use crate::theme::BuddyPalette;
use crate::zoom::zpx;

pub const ACTIVITY_BAR_WIDTH: f32 = 48.0;

struct SectionItem {
    section: Section,
    label: &'static str,
    icon: IconName,
}

fn sections() -> [SectionItem; 11] {
    [
        SectionItem {
            section: Section::GitHub,
            label: "GitHub",
            icon: IconName::Github,
        },
        SectionItem {
            section: Section::Terminal,
            label: "Terminal",
            icon: IconName::SquareTerminal,
        },
        SectionItem {
            section: Section::Tasks,
            label: "Tasks",
            icon: IconName::SquareCheck,
        },
        SectionItem {
            section: Section::Insights,
            label: "Insights",
            icon: IconName::ChartColumn,
        },
        SectionItem {
            section: Section::Automation,
            label: "Automation",
            icon: IconName::Bot,
        },
        SectionItem {
            section: Section::Ralph,
            label: "Ralph Loops",
            icon: IconName::RefreshCw,
        },
        SectionItem {
            section: Section::Crew,
            label: "The Crew",
            icon: IconName::Users,
        },
        SectionItem {
            section: Section::Tempo,
            label: "Tempo",
            icon: IconName::Clock,
        },
        SectionItem {
            section: Section::Bookmarks,
            label: "Bookmarks",
            icon: IconName::Bookmark,
        },
        SectionItem {
            section: Section::Copilot,
            label: "Copilot",
            icon: IconName::Sparkles,
        },
        SectionItem {
            section: Section::Settings,
            label: "Settings",
            icon: IconName::Settings,
        },
    ]
}

fn item(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    active: bool,
    cx: &mut Context<BuddyApp>,
    on_click: impl Fn(&mut BuddyApp, &mut Window, &mut Context<BuddyApp>) + 'static,
) -> AnyElement {
    let palette = *BuddyPalette::global(cx);
    let accent = cx.theme().primary;
    // A real button: focusable, keyboard-operable, and announced by its label.
    div()
        .w_full()
        .h(zpx(48.0))
        .flex()
        .items_center()
        .justify_center()
        .border_l_2()
        .border_color(if active {
            accent
        } else {
            gpui_kit::transparent_black()
        })
        .when(active, |this| this.bg(palette.activity_bar_hover))
        .child(
            Button::new(id)
                .ghost()
                .compact()
                .w(zpx(44.0))
                .h(zpx(44.0))
                .text_color(if active {
                    palette.activity_bar_fg_active
                } else {
                    palette.activity_bar_fg
                })
                .icon(Icon::new(icon).size(zpx(24.0)))
                .tooltip(label)
                .accessibility_label(label)
                .selected(active)
                .on_click(cx.listener(move |this, _, window, cx| {
                    on_click(this, window, cx);
                    cx.notify();
                })),
        )
        .into_any_element()
}

pub fn render(app: &BuddyApp, cx: &mut Context<BuddyApp>) -> impl IntoElement + use<> {
    let palette = *BuddyPalette::global(cx);
    let dashboard_active = app.active_section.is_none();

    // Scrolls when the window is shorter than the twelve 48px entries
    // (Dashboard plus eleven sections) need.
    v_flex()
        .id("activity-bar")
        .w(zpx(ACTIVITY_BAR_WIDTH))
        .flex_shrink_0()
        .h_full()
        .overflow_y_scroll()
        .bg(palette.activity_bar_bg)
        .child(item(
            "activity-home",
            "Dashboard",
            IconName::LayoutDashboard,
            dashboard_active,
            cx,
            |this, _, cx| this.open_section(None, cx),
        ))
        .child(
            div()
                .mx(zpx(8.0))
                .my(zpx(4.0))
                .h(zpx(1.0))
                .bg(palette.border_secondary),
        )
        .children(sections().into_iter().map(|entry| {
            let section = entry.section;
            item(
                section.id(),
                entry.label,
                entry.icon,
                app.active_section == Some(section),
                cx,
                move |this, _, cx| this.open_section(Some(section), cx),
            )
        }))
}
