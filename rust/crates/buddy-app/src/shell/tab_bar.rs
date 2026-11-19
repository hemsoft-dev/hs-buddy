//! Global workspace tabs, independent of the embedded browser's internal pages.
use crate::{app::BuddyApp, zoom::zpx};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Selectable as _, Sizable as _, h_flex};
use gpui_kit::{
    Context, DismissEvent, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, WeakEntity, div,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
pub const TAB_BAR_HEIGHT: f32 = 35.0;
type Overlay = Rc<Cell<bool>>;
type OverlaySlot = Rc<RefCell<Option<Overlay>>>;

#[derive(Default)]
struct TabScroll {
    handle: gpui_kit::ScrollHandle,
    active: Option<String>,
    keys: Vec<String>,
    width: gpui_kit::Pixels,
}

fn tab_menu(
    mut menu: PopupMenu,
    owner: WeakEntity<BuddyApp>,
    key: &str,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(app) = owner.upgrade() else {
        return menu;
    };
    let tabs = &app.read(cx).tabs;
    let Some(index) = tabs.items.iter().position(|tab| tab.key == key) else {
        return menu;
    };
    let count = tabs.items.len();
    for (label, action, disabled) in [
        ("Close", 0, false),
        ("Close Others", 1, count <= 1),
        ("Close to the Right", 2, index + 1 == count),
        ("Close All", 3, false),
        ("Move Left", 4, false),
        ("Move Right", 5, false),
    ] {
        let owner = owner.clone();
        let key = key.to_string();
        menu = menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(
            move |_, window, cx| {
                let _ = owner.update(cx, |app, cx| {
                    // The target can disappear while a popup is open. In
                    // particular, a stale Close All must not close new tabs.
                    if !app.tabs.items.iter().any(|tab| tab.key == key) {
                        return;
                    }
                    match action {
                        0 => app.tabs.close(&key),
                        1 => app.tabs.retain(&key, false),
                        2 => app.tabs.retain(&key, true),
                        3 => app.tabs.close_all(),
                        4 => app.tabs.move_tab(&key, -1),
                        _ => app.tabs.move_tab(&key, 1),
                    }
                    app.restore_tab(window, cx);
                });
            },
        ));
    }
    menu
}

fn set_overlay(open: &Cell<bool>, value: bool, cx: &mut gpui_kit::App) {
    if open.replace(value) != value {
        crate::browser_view::Overlays::set(value, cx);
    }
}

/// A bulk close can remove the trigger before its open-change callback runs.
/// Every menu therefore releases its guard on dismissal and on entity release.
fn menu_overlay(owner: WeakEntity<BuddyApp>, open: Overlay, cx: &mut Context<PopupMenu>) {
    let release_open = open.clone();
    let release_owner = owner.clone();
    cx.on_release(move |_, cx| {
        set_overlay(&release_open, false, cx);
        let _ = release_owner.update(cx, |_, cx| cx.notify());
    })
    .detach();
    let menu = cx.entity();
    cx.subscribe(&menu, move |_, _, _: &DismissEvent, cx| {
        set_overlay(&open, false, cx);
        let _ = owner.update(cx, |_, cx| cx.notify());
    })
    .detach();
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
        let context_key = key.clone();
        let weak = cx.entity().downgrade();
        let context_owner = weak.clone();
        let dropdown_open = window
            .use_keyed_state(
                gpui_kit::ElementId::Name(format!("tab-menu-guard:{}", tab.key).into()),
                cx,
                |_, _| OverlaySlot::default(),
            )
            .read(cx)
            .clone();
        let dropdown_release = dropdown_open.clone();
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
                        .dropdown_menu(move |menu, _, cx| {
                            // A released previous menu must not clear the
                            // newly opened menu's guard during a rapid reopen.
                            let open = Overlay::default();
                            if let Some(previous) =
                                dropdown_release.borrow_mut().replace(open.clone())
                            {
                                set_overlay(&previous, false, cx);
                            }
                            // Dropdown content is built only when shown; its
                            // open-change notification can precede this build.
                            set_overlay(&open, true, cx);
                            menu_overlay(weak.clone(), open, cx);
                            tab_menu(menu, weak.clone(), &menu_key, cx)
                        })
                        .on_open_change(move |open, _, cx| {
                            if let Some(current) = dropdown_open.borrow().as_ref() {
                                set_overlay(current, *open, cx);
                            }
                        }),
                )
                .context_menu(move |menu, _, cx| {
                    let open = Rc::new(Cell::new(false));
                    set_overlay(&open, true, cx);
                    menu_overlay(context_owner.clone(), open, cx);
                    tab_menu(menu, context_owner.clone(), &context_key, cx)
                }),
        );
    }
    strip = strip.child(tabs).child(div().w(zpx(2.0)));
    strip
}
