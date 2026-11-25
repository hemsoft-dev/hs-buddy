//! The worklog deletion confirmation. Timeline's Delete worklog and a grid
//! right-click both set `delete_target`. The bar is pinned above the view's
//! scrolling page, so it stays in view however far the page is scrolled. It
//! belongs to this view alone: it leaves with the view, and nothing outside
//! the view keeps track of it.
use super::render::{delete_prompt, duration, status_block};
use super::*;
use crate::zoom::zpx;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, TestSupportExt as _, div,
};

impl TempoView {
    /// Timeline's Delete worklog and a grid right-click share this prompt.
    /// Keyboard focus moves to the bar, so Escape reaches it at once.
    pub(super) fn ask_delete(
        &mut self,
        row: buddy_core::tempo::Worklog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The confirmation replaces any stale status.
        self.delete_target = Some(row);
        self.delete_unconfirmed = false;
        self.delete_sending = false;
        self.error = None;
        self.notice = None;
        window.focus(&self.delete_focus, cx);
        cx.notify();
    }

    /// Keep worklog, Escape, or Close once the deletion's result is unknown.
    /// A delete in flight finishes first.
    fn dismiss_delete(&mut self, cx: &mut Context<Self>) {
        if self.busy && self.delete_sending {
            return;
        }
        self.delete_target = None;
        self.delete_unconfirmed = false;
        cx.notify();
    }

    /// The pinned bar for `delete_target`, if any. Its buttons follow the
    /// view's busy and write state, and a failed delete shows its error here.
    pub(super) fn render_delete_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.delete_target.as_ref()?;
        let sending = self.busy && self.delete_sending;
        let unconfirmed = self.delete_unconfirmed;
        let mut lines = vec![if unconfirmed {
            (
                Tone::Warning,
                unconfirmed_delete(target.seconds, target.date),
            )
        } else {
            delete_prompt(target.seconds, target.date)
        }];
        if let Some(description) = target.description.clone().filter(|text| !text.is_empty()) {
            lines.push((Tone::Info, description));
        }
        if let Some(error) = &self.error {
            lines.push((Tone::Error, error.clone()));
        }
        let dismiss = |id: &'static str, label: &'static str| {
            Button::new(id)
                .label(label)
                .disabled(sending)
                .on_click(cx.listener(|this, _, _, cx| this.dismiss_delete(cx)))
        };
        // An unconfirmed delete may already have removed the worklog, so the
        // bar neither offers it again nor claims it is kept.
        let actions = if unconfirmed {
            h_flex()
                .gap_2()
                .child(dismiss("tempo-close-unconfirmed-delete", "Close"))
        } else {
            h_flex()
                .gap_2()
                .child(
                    Button::new("tempo-confirm-delete")
                        .label("Confirm deletion")
                        .danger()
                        .disabled(!self.can_submit())
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
                )
                .child(dismiss("tempo-keep-worklog", "Keep worklog"))
        }
        .into_any_element();
        let body = status_block(lines, vec![actions], cx);
        let theme = cx.theme();
        let title = if unconfirmed {
            "Deletion unconfirmed"
        } else {
            "Delete worklog"
        };
        let progress = sending.then(|| {
            div()
                .id("tempo-delete-progress")
                .test_support()
                .text_color(theme.muted_foreground)
                .child("Deleting the worklog in Tempo…")
        });
        Some(
            v_flex()
                .id("tempo-delete-bar")
                .test_support()
                .track_focus(&self.delete_focus)
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.dismiss_delete(cx);
                    }
                }))
                .flex_shrink_0()
                .gap_2()
                .px(zpx(16.))
                .py(zpx(12.))
                .bg(theme.background)
                .border_b_1()
                .border_color(theme.danger)
                .child(
                    div()
                        .id("tempo-delete-title")
                        .aria_label(title)
                        .test_support()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(body)
                .children(progress)
                .into_any_element(),
        )
    }
}

/// Said in place of the prompt once Tempo may have deleted the worklog
/// without replying, in the words of the page's reconciliation warning.
pub(super) fn unconfirmed_delete(seconds: u64, date: impl std::fmt::Display) -> String {
    format!(
        "Tempo may already have deleted the {} worklog on {date}. It will not resend automatically. Refresh and inspect the worklogs before discarding local recovery.",
        duration(seconds)
    )
}
