//! Port of `TitleBar.tsx`. The window is frameless, so File/Edit/View/Help
//! live here (see the Frameless Window rule in AGENTS.md). Edit items dispatch
//! the component library's own input actions so they act on the focused field.

use gpui_kit::assets::IconName;
use gpui_kit::base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{ActiveTheme, Icon, Selectable as _, Sizable as _, TitleBar, h_flex};
use gpui_kit::{
    Context, FontWeight, IntoElement, ParentElement, Styled, Window, div, linear_color_stop,
    linear_gradient,
};

use crate::app::{About, BuddyApp, Quit, Reload, ResetZoom, ToggleFullScreen, ZoomIn, ZoomOut};
use crate::settings::Settings;
use crate::theme::BuddyPalette;
use crate::zoom::{self, scaled, zpx};

pub const TITLE_BAR_HEIGHT: f32 = 34.0;

fn app_icon(palette: &BuddyPalette, background: gpui_kit::Hsla) -> impl IntoElement {
    div()
        .size(zpx(20.0))
        .flex_shrink_0()
        .rounded(zpx(5.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(palette.gold, 0.0),
            linear_color_stop(palette.orange, 1.0),
        ))
        .text_color(background)
        .child(Icon::new(IconName::Users).size(zpx(13.0)))
}

fn menu_button(id: &'static str, label: &'static str) -> Button {
    Button::new(id).ghost().xsmall().label(label).compact()
}

pub fn render(
    _app: &BuddyApp,
    _window: &mut Window,
    cx: &mut Context<BuddyApp>,
) -> impl IntoElement + use<> {
    let palette = *BuddyPalette::global(cx);
    let (assistant_open, terminal_open) = {
        let ui = &Settings::global(cx).config.ui;
        (ui.assistant_open, ui.terminal_open)
    };
    let theme = cx.theme();
    let title_bar_bg = theme.title_bar;
    let foreground = theme.foreground;
    let background = theme.background;

    let content = h_flex()
        .flex_1()
        .h_full()
        .items_center()
        .gap(zpx(10.0))
        .text_size(zpx(12.0))
        .text_color(foreground)
        .child(app_icon(&palette, background))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_heading)
                .child("Buddy"),
        )
        .child(
            h_flex()
                .gap(zpx(2.0))
                .child(
                    menu_button("menu-file", "File")
                        .dropdown_menu(|menu, _, _| menu.menu("Exit", Box::new(Quit)))
                        .on_open_change(menu_overlay),
                )
                .child(
                    menu_button("menu-edit", "Edit")
                        .dropdown_menu(|menu, _, _| {
                            menu.menu("Undo", Box::new(Undo))
                                .menu("Redo", Box::new(Redo))
                                .separator()
                                .menu("Cut", Box::new(Cut))
                                .menu("Copy", Box::new(Copy))
                                .menu("Paste", Box::new(Paste))
                                .separator()
                                .menu("Select All", Box::new(SelectAll))
                        })
                        .on_open_change(menu_overlay),
                )
                .child(
                    menu_button("menu-view", "View")
                        .dropdown_menu(|menu, _, _| {
                            menu.menu("Zoom In", Box::new(ZoomIn))
                                .menu("Zoom Out", Box::new(ZoomOut))
                                .menu("Reset Zoom", Box::new(ResetZoom))
                                .separator()
                                .menu("Reload Configuration", Box::new(Reload))
                                .separator()
                                .menu("Full Screen", Box::new(ToggleFullScreen))
                        })
                        .on_open_change(menu_overlay),
                )
                .child(
                    menu_button("menu-help", "Help")
                        .dropdown_menu(|menu, _, _| menu.menu("About Buddy", Box::new(About)))
                        .on_open_change(menu_overlay),
                ),
        )
        .child(div().flex_1())
        .child(
            h_flex()
                .gap(zpx(2.0))
                .pr(zpx(6.0))
                .child(
                    Button::new("toggle-assistant")
                        .ghost()
                        .xsmall()
                        .compact()
                        .icon(Icon::new(IconName::Sparkles).size(zpx(14.0)))
                        .accessibility_label("Toggle Copilot Assistant")
                        .tooltip("Toggle Copilot Assistant (panel not ported yet)")
                        .selected(assistant_open)
                        .on_click(cx.listener(|_, _, _, cx| {
                            // The state this click asks for, decided
                            // once: a retried save must not toggle a
                            // value Electron changed meanwhile.
                            let open = !Settings::global(cx).config.ui.assistant_open;
                            Settings::update(cx, move |config| {
                                config.ui.assistant_open = open;
                            });
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("toggle-terminal")
                        .ghost()
                        .xsmall()
                        .compact()
                        .icon(Icon::new(IconName::SquareTerminal).size(zpx(14.0)))
                        .accessibility_label("Toggle Terminal")
                        .tooltip("Toggle Terminal (panel not ported yet)")
                        .selected(terminal_open)
                        .on_click(cx.listener(|_, _, _, cx| {
                            let open = !Settings::global(cx).config.ui.terminal_open;
                            Settings::update(cx, move |config| {
                                config.ui.terminal_open = open;
                            });
                            cx.notify();
                        })),
                ),
        );

    // The window controls are gpui-component's, 34px wide whatever the rem
    // size, so the bar itself stays at 100% (controls keep their fixed size,
    // as in VS Code) while its height and Buddy's content follow the zoom.
    let title_bar_height = scaled(cx, TITLE_BAR_HEIGHT);
    zoom::unzoomed(
        TitleBar::new()
            .h(title_bar_height)
            .bg(title_bar_bg)
            .child(zoom::zoomed(cx, content)),
    )
}

/// An open menu drops over the content area, where a web page would hide it.
fn menu_overlay(open: &bool, _: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    crate::browser_view::Overlays::set(*open, cx);
}
