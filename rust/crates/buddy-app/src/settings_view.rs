//! Application settings, using the shell's existing theme and config edits.

use buddy_core::application_settings::LocalEdit;
use buddy_core::config::ThemeName;
use gpui_kit::base::Selectable;
use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
};

use crate::settings::Settings;
use crate::zoom::zpx;

pub struct SettingsView {
    status: Option<String>,
}

impl SettingsView {
    pub fn new() -> Self {
        Self { status: None }
    }

    fn apply(&mut self, edit: LocalEdit, cx: &mut Context<Self>) {
        self.status = Some(
            match Settings::try_update(cx, |config| edit.apply(config)) {
                Ok(()) => "Saved".into(),
                Err(error) => error.to_string(),
            },
        );
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Settings::global(cx).config.ui.theme;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("application-settings")
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .p_6()
            .gap_4()
            .text_color(foreground)
            .child(div().text_size(zpx(24.0)).child("Settings"))
            .child(div().text_color(muted).child("Appearance"))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("settings-dark-theme")
                            .label("Dark")
                            .selected(theme == ThemeName::Dark)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply(LocalEdit::Theme(ThemeName::Dark), cx)
                            })),
                    )
                    .child(
                        Button::new("settings-light-theme")
                            .label("Light")
                            .selected(theme == ThemeName::Light)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply(LocalEdit::Theme(ThemeName::Light), cx)
                            })),
                    ),
            )
            .children(self.status.clone().map(|status| div().child(status)))
    }
}
