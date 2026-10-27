//! Tab strip, toolbar, notices, and the area a native page is placed over.
use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div,
};

use super::{BrowserView, CONTEXT, CloseTab, NextTab, Notice, PreviousTab, ZOOM_DELTA};
use crate::zoom::zpx;

impl Render for BrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active {
            None => self.render_home(cx),
            Some(index) => self.render_tab(index, cx),
        };
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.cycle(false, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_active(cx)))
            .debug_selector(|| "browser-view".into())
            .child(self.render_strip(cx))
            .when(self.full, |this| {
                this.child(self.render_notice(
                    Notice {
                        text: format!(
                            "Buddy keeps up to {} tabs. Close one to open another.",
                            buddy_core::browser::MAX_TABS
                        ),
                        offer: None,
                    },
                    false,
                    cx,
                ))
            })
            .child(content)
    }
}

impl BrowserView {
    fn render_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let mut strip = h_flex()
            .id("browser-tabs")
            .w_full()
            .flex_shrink_0()
            .overflow_x_scroll()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                tab_frame(self.active.is_none(), cx).child(
                    // Buttons, so Tab reaches every tab and Enter or Space selects it.
                    Button::new("browser-home")
                        .icon(IconName::Bookmark)
                        .label("Bookmarks")
                        .ghost()
                        .small()
                        .selected(self.active.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                ),
            );
        for (index, tab) in self.tabs.iter().enumerate() {
            let full: SharedString = tab.label().to_string().into();
            strip = strip.child(
                tab_frame(self.active == Some(index), cx)
                    .child(
                        Button::new(("browser-tab", tab.id))
                            .icon(IconName::Globe)
                            .label(short(&full))
                            .ghost()
                            .small()
                            .selected(self.active == Some(index))
                            .tooltip(full.clone())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.select(Some(index), cx)),
                            ),
                    )
                    .child(
                        Button::new(("browser-close", tab.id))
                            .icon(IconName::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Close tab")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close(index, cx);
                            })),
                    ),
            );
        }
        strip
    }

    fn render_home(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.home {
            Some(home) => div()
                .flex_1()
                .min_h_0()
                .child(home.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .p_4()
                .text_color(cx.theme().muted_foreground)
                .child("Bookmarks open here.")
                .into_any_element(),
        }
    }

    fn render_tab(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let tab = &self.tabs[index];
        let id = tab.id;
        let view = cx.entity();
        let area = if tab.page.is_some() || tab.starting || tab.notice.is_none() {
            // The native page covers exactly this area once it exists.
            div()
                .id(("browser-page", id))
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            view.update(cx, |this, cx| {
                                if this.place(id, bounds) {
                                    this.sync(cx);
                                }
                            })
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                )
                .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(self.render_toolbar(index, cx))
            .when_some(tab.notice.clone(), |this, notice| {
                this.child(self.render_notice(notice, tab.page.is_some(), cx))
            })
            .child(area)
            .into_any_element()
    }

    fn render_toolbar(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = &self.tabs[index];
        let live = tab.page.is_some();
        let theme = cx.theme();
        h_flex()
            .w_full()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("browser-back")
                    .icon(IconName::ArrowLeft)
                    .ghost()
                    .small()
                    .tooltip("Back")
                    .disabled(!live)
                    .on_click(cx.listener(|this, _, _, _| this.back())),
            )
            .child(
                Button::new("browser-forward")
                    .icon(IconName::ArrowRight)
                    .ghost()
                    .small()
                    .tooltip("Forward")
                    .disabled(!live)
                    .on_click(cx.listener(|this, _, _, _| this.forward())),
            )
            .child(
                Button::new("browser-reload")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .small()
                    .tooltip(if live { "Reload" } else { "Try again" })
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.secondary)
                    .text_size(zpx(12.0))
                    .when(tab.loading, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_color(theme.muted_foreground)
                                .child("Loading…"),
                        )
                    })
                    .child(
                        div()
                            .id("browser-url")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(tab.url.clone())),
                    ),
            )
            .child(
                Button::new("browser-zoom-out")
                    .icon(IconName::Minus)
                    .ghost()
                    .small()
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(-ZOOM_DELTA, cx))),
            )
            .child(
                div()
                    .text_size(zpx(12.0))
                    .text_color(theme.muted_foreground)
                    .child(format!("{:.0}%", self.zoom * 100.0)),
            )
            .child(
                Button::new("browser-zoom-in")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_DELTA, cx))),
            )
            .child(
                Button::new("browser-external")
                    .icon(IconName::ExternalLink)
                    .ghost()
                    .small()
                    .tooltip("Open in your browser")
                    .on_click(cx.listener(|this, _, _, cx| this.open_externally(cx))),
            )
    }

    fn render_notice(
        &self,
        notice: Notice,
        live: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let mailto = notice
            .offer
            .as_ref()
            .is_some_and(|url| url.scheme() == "mailto");
        h_flex()
            .id("browser-notice")
            .debug_selector(|| "browser-notice".into())
            .w_full()
            .flex_shrink_0()
            .gap_2()
            .px_3()
            .py_2()
            .bg(theme.warning.opacity(0.12))
            .border_b_1()
            .border_color(theme.warning)
            .child(Icon::new(IconName::TriangleAlert).size(zpx(16.0)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(zpx(13.0))
                    .child(SharedString::from(notice.text)),
            )
            .when(notice.offer.is_some(), |this| {
                this.child(
                    Button::new("browser-notice-open")
                        .label(if mailto {
                            "Open in mail app"
                        } else {
                            "Open in browser"
                        })
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.open_offer(cx))),
                )
            })
            .when(live, |this| {
                this.child(
                    Button::new("browser-notice-dismiss")
                        .icon(IconName::X)
                        .ghost()
                        .small()
                        .tooltip("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                )
            })
    }
}

/// A tab's frame in the strip; the selected one is marked like the shell's tabs.
fn tab_frame(selected: bool, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    h_flex()
        .h(zpx(32.0))
        .flex_shrink_0()
        .gap_1()
        .px_1()
        .items_center()
        .border_r_1()
        .border_color(theme.border)
        .when(selected, |this| {
            this.bg(theme.tab_active)
                .border_b_1()
                .border_color(theme.primary)
        })
}

/// A page title short enough for the strip; the tooltip has all of it.
fn short(label: &str) -> SharedString {
    const MAX: usize = 28;
    if label.chars().count() <= MAX {
        return label.to_string().into();
    }
    let kept: String = label.chars().take(MAX - 1).collect();
    format!("{}\u{2026}", kept.trim_end()).into()
}
