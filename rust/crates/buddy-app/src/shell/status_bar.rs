//! Port of `StatusBar.tsx`: compact status items on the configured colors.
//! Only metrics the native app can actually provide are shown.

use gpui_kit::assets::IconName;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::{App, IntoElement, ParentElement, SharedString, Styled};

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

pub fn render(app: &BuddyApp, cx: &App) -> impl IntoElement + use<> {
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

    StatusBar::new()
        .h(zpx(STATUS_BAR_HEIGHT))
        .bg(theme.status_bar)
        .border_t_1()
        .border_color(theme.status_bar_border)
        .text_color(palette.status_bar_fg)
        .text_size(zpx(11.0))
        .left(status_item(IconName::Users, account))
        .left(gpui_kit::div().children(
            save_error.map(|error| {
                status_item(IconName::Settings, format!("Settings not saved: {error}"))
            }),
        ))
        .right(status_item(IconName::Clock, clock))
}
