//! Port of `AboutModal.tsx`: the Help > About Buddy dialog.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div,
    linear_color_stop, linear_gradient,
};

use crate::theme::BuddyPalette;
use crate::zoom::zpx;

/// Where "View on GitHub" leads. Electron still names the repository's
/// earlier home (`relias-engineering/hs-buddy`); it now lives here.
pub const REPOSITORY_URL: &str = "https://github.com/hemsoft-dev/hs-buddy";

pub const DESCRIPTION: &str = "A powerful desktop app for managing GitHub PRs, automating workflows, and boosting productivity with the HemSoft skills infrastructure.";

/// The `.about-tech-stack` tiles, naming what this build is made of.
pub const TECH_STACK: [(&str, &str); 3] = [
    ("RUNTIME", "Native (Rust)"),
    ("FRAMEWORK", "GPUI"),
    ("BUILD", "Cargo"),
];

fn tech_tile(label: &'static str, value: &'static str, palette: &BuddyPalette) -> impl IntoElement {
    v_flex()
        .flex_1()
        .items_center()
        .gap(zpx(2.0))
        .px(zpx(8.0))
        .py(zpx(6.0))
        .rounded(zpx(6.0))
        .bg(palette.bg_tertiary)
        .child(
            div()
                .text_size(zpx(10.0))
                .text_color(palette.text_muted)
                .child(label),
        )
        .child(
            div()
                .text_size(zpx(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_heading)
                .child(value),
        )
}

fn content(cx: &App) -> impl IntoElement + use<> {
    let palette = *BuddyPalette::global(cx);
    v_flex()
        .w_full()
        .items_center()
        .gap(zpx(12.0))
        .pb(zpx(8.0))
        .child(
            div()
                .size(zpx(72.0))
                .rounded(zpx(16.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(linear_gradient(
                    135.0,
                    linear_color_stop(palette.gold, 0.0),
                    linear_color_stop(palette.orange, 1.0),
                ))
                .child(Icon::new(IconName::Users).size(zpx(40.0))),
        )
        .child(
            div()
                .text_size(zpx(24.0))
                .font_weight(FontWeight::BOLD)
                .text_color(palette.text_heading)
                .child("Buddy"),
        )
        .child(
            div()
                .px(zpx(8.0))
                .py(zpx(2.0))
                .rounded(zpx(10.0))
                .bg(palette.gold.opacity(0.12))
                .text_size(zpx(12.0))
                .text_color(palette.gold)
                .child(SharedString::from(format!(
                    "Version {}",
                    env!("BUDDY_VERSION")
                ))),
        )
        .child(
            h_flex()
                .gap(zpx(6.0))
                .text_color(palette.text_secondary)
                .child("🤝")
                .child("Your Universal Productivity Companion"),
        )
        .child(
            div()
                .max_w(zpx(360.0))
                .text_center()
                .text_size(zpx(13.0))
                .text_color(palette.text_secondary)
                .child(DESCRIPTION),
        )
        .child(
            h_flex().w_full().gap(zpx(8.0)).children(
                TECH_STACK
                    .iter()
                    .map(|(label, value)| tech_tile(label, value, &palette)),
            ),
        )
        .child(
            Button::new("about-github")
                .outline()
                .small()
                .icon(IconName::Github)
                .label("View on GitHub")
                .on_click(|_, _, cx| cx.open_url(REPOSITORY_URL)),
        )
        .child(
            h_flex()
                .gap(zpx(5.0))
                .text_size(zpx(11.0))
                .text_color(palette.text_muted)
                .child("Made with")
                .child(
                    Icon::new(IconName::Heart)
                        .size(zpx(12.0))
                        .text_color(crate::theme::hex("#e25555")),
                )
                .child("by HemSoft Developments"),
        )
}

/// Help > About Buddy. The title-bar menu dispatches this while its window
/// is still handling the click, when that window cannot be updated again, so
/// the dialog opens once the current update has finished.
pub fn show(cx: &mut App) {
    cx.defer(|cx| {
        if let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied())
            && let Err(error) = window.update(cx, |_, window, cx| open(window, cx))
        {
            log::warn!("About Buddy could not open: {error}");
        }
    });
}

/// Opens the dialog; Escape and the close button dismiss it.
pub fn open(window: &mut Window, cx: &mut App) {
    super::dialogs::open(
        window,
        cx,
        |dialog, _, cx| {
            dialog
                .w(gpui_kit::px(420.0))
                .title("About Buddy")
                .child(content(cx))
        },
        |_, _| {},
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;

    #[gpui_kit::test]
    fn about_shows_the_build_and_opens_the_repository(cx: &mut gpui_kit::TestAppContext) {
        let window = cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Empty)).unwrap()
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(window.0, cx);
        visual.update(|window, cx| {
            open(window, cx);
            window.draw(cx).clear(cx);
            window.click("about-github", cx);
        });
        assert_eq!(visual.opened_url().as_deref(), Some(REPOSITORY_URL));
    }

    /// Dispatched from inside its window, as the title-bar menu does.
    #[gpui_kit::test]
    fn about_opens_from_a_menu_inside_its_window(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::WindowExt as _;
        let window = cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Empty)).unwrap()
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(window.0, cx);
        visual.update(|window, cx| {
            assert!(!window.has_active_dialog(cx));
            show(cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    struct Empty;
    impl gpui_kit::Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
            div()
        }
    }
}
