//! Global workspace tabs, independent of the embedded browser's internal pages.
use crate::{app::BuddyApp, zoom::zpx};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Selectable as _, Sizable as _, h_flex};
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div,
};
pub const TAB_BAR_HEIGHT: f32 = 35.0;

#[derive(Default)]
struct TabScroll {
    handle: gpui_kit::ScrollHandle,
    active: Option<String>,
    keys: Vec<String>,
    width: gpui_kit::Pixels,
}

pub fn render(
    app: &BuddyApp,
    window: &mut gpui_kit::Window,
    cx: &mut Context<BuddyApp>,
) -> impl IntoElement + use<> {
    let width = window.viewport_size().width;
    let state = window.use_keyed_state("global-tab-scroll", cx, |_, _| TabScroll::default());
    let scroll = state.update(cx, |state, _| {
        let keys: Vec<_> = app.tabs.items.iter().map(|d| d.key.clone()).collect();
        if state.active != app.tabs.active || state.keys != keys || state.width != width {
            if let Some(index) = app
                .tabs
                .items
                .iter()
                .position(|d| Some(&d.key) == app.tabs.active.as_ref())
            {
                state.handle.scroll_to_item(index);
            }
            state.active = app.tabs.active.clone();
            state.keys = keys;
            state.width = width;
        }
        state.handle.clone()
    });
    let focuses: Vec<_> = app
        .tabs
        .items
        .iter()
        .map(|tab| {
            window
                .use_keyed_state(
                    gpui_kit::ElementId::Name(format!("tab-focus:{}", tab.key).into()),
                    cx,
                    |_, cx| cx.focus_handle(),
                )
                .read(cx)
                .clone()
        })
        .collect();
    let mut strip = h_flex()
        .id("global-tabs")
        .w_full()
        .h(zpx(TAB_BAR_HEIGHT))
        .flex_shrink_0()
        .bg(cx.theme().tab_bar)
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            Button::new("navigation-back")
                .ghost()
                .xsmall()
                .label("‹")
                .tooltip("Back (Alt+Left)")
                .accessibility_label("Navigation back")
                .disabled(!app.tabs.can_back())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.tabs.travel(false);
                    this.restore_tab(window, cx);
                })),
        )
        .child(
            Button::new("navigation-forward")
                .ghost()
                .xsmall()
                .label("›")
                .tooltip("Forward (Alt+Right)")
                .accessibility_label("Navigation forward")
                .disabled(!app.tabs.can_forward())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.tabs.travel(true);
                    this.restore_tab(window, cx);
                })),
        );
    let mut tabs = h_flex()
        .id("global-tab-list")
        .role(gpui_kit::Role::TabList)
        .flex_1()
        .min_w_0()
        .overflow_x_scroll()
        .track_scroll(&scroll)
        .h_full();
    for (index, tab) in app.tabs.items.iter().enumerate() {
        let key = tab.key.clone();
        let keyboard_key = key.clone();
        let focus = focuses[index].clone();
        let key_focus = focus.clone();
        let select_focus = focus.clone();
        let other_focus = focuses.clone();
        let keyboard_scroll = scroll.clone();
        let close = key.clone();
        let menu_key = key.clone();
        let weak = cx.entity().downgrade();
        let active = app.tabs.active.as_ref() == Some(&key);
        tabs = tabs.child(
            h_flex()
                .id(("global-tab-frame", index))
                .role(gpui_kit::Role::Tab)
                .aria_label(tab.label.clone())
                .aria_selected(active)
                .track_focus(&focus)
                .on_key_down(cx.listener(
                    move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                        if event.keystroke.modifiers != gpui_kit::Modifiers::default() {
                            return;
                        }
                        let target = match event.keystroke.key.as_str() {
                            "right" => Some((index + 1) % other_focus.len()),
                            "left" => Some((index + other_focus.len() - 1) % other_focus.len()),
                            "home" => Some(0),
                            "end" => Some(other_focus.len() - 1),
                            _ => None,
                        };
                        if let Some(target) = target {
                            keyboard_scroll.scroll_to_item(target);
                            window.focus(&other_focus[target], cx);
                            cx.stop_propagation();
                        } else if key_focus.is_focused(window)
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        {
                            this.tabs.select(&keyboard_key);
                            this.restore_tab(window, cx);
                            window.focus(&key_focus, cx);
                            cx.stop_propagation();
                        }
                    },
                ))
                .h_full()
                .border_r_1()
                .border_b_2()
                .border_color(if active {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .bg(if active {
                    cx.theme().tab_active
                } else {
                    cx.theme().tab_bar
                })
                .child(
                    Button::new(("global-tab", index))
                        .ghost()
                        .xsmall()
                        .label(tab.label.clone())
                        .tooltip(tab.label.clone())
                        .accessibility_label(format!("Select {} tab", tab.label))
                        .selected(active)
                        .tab_stop(active)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.tabs.select(&key);
                            this.restore_tab(window, cx);
                            window.focus(&select_focus, cx);
                        })),
                )
                .child(
                    Button::new(("close-global-tab", index))
                        .ghost()
                        .xsmall()
                        .label("×")
                        .accessibility_label(format!("Close {} tab", tab.label))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.tabs.close(&close);
                            this.restore_tab(window, cx);
                        })),
                )
                .child(
                    Button::new(("global-tab-menu", index))
                        .ghost()
                        .xsmall()
                        .label("⋯")
                        .accessibility_label(format!("{} tab actions", tab.label))
                        .dropdown_menu(move |mut menu, _, _| {
                            for (label, action) in [
                                ("Close", 0),
                                ("Close Others", 1),
                                ("Close to the Right", 2),
                                ("Close All", 3),
                                ("Move Left", 4),
                                ("Move Right", 5),
                            ] {
                                let weak = weak.clone();
                                let key = menu_key.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        weak.update(cx, |this, cx| {
                                            match action {
                                                0 => this.tabs.close(&key),
                                                1 => this.tabs.retain(&key, false),
                                                2 => this.tabs.retain(&key, true),
                                                3 => this.tabs.close_all(),
                                                4 => this.tabs.move_tab(&key, -1),
                                                _ => this.tabs.move_tab(&key, 1),
                                            }
                                            this.restore_tab(window, cx);
                                        })
                                        .ok();
                                    },
                                ));
                            }
                            menu
                        })
                        .on_open_change(|open, _, cx| {
                            crate::browser_view::Overlays::set(*open, cx)
                        }),
                ),
        );
    }
    strip = strip.child(tabs).child(div().w(zpx(2.0)));
    strip
}
