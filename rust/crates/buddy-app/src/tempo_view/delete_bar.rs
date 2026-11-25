//! The worklog deletion confirmation. Timeline's Delete worklog and a grid
//! right-click both set `delete_target`. The bar is pinned above the view's
//! scrolling page, so it stays in view however far the page is scrolled. It
//! belongs to this view alone: it leaves with the view, and nothing outside
//! the view keeps track of it.
use super::render::{
    RecoveryIds, delete_prompt, discard_prompt, duration, recovery_line, status_block,
};
use super::*;
use crate::zoom::zpx;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
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
        self.error = None;
        self.notice = None;
        window.focus(&self.delete_focus, cx);
        cx.notify();
    }

    /// Keep worklog, Escape, or Close once the deletion's result is unknown.
    /// A delete of this worklog in flight finishes first, whichever control
    /// sent it.
    fn dismiss_delete(&mut self, cx: &mut Context<Self>) {
        if self.delete_in_flight() {
            return;
        }
        self.delete_target = None;
        self.delete_unconfirmed = false;
        cx.notify();
    }

    /// The pinned bar for `delete_target`, if any. Its buttons follow the
    /// view's busy and write state, and a failed delete shows its error here.
    /// The title and buttons share the bar's first row, so no text can push
    /// them out of view: the description is one truncated line with the
    /// rest in its tooltip, and the messages scroll within a capped height.
    /// In a pane too short for the whole bar, the messages shrink first and
    /// the bar clips below its buttons. A captured change of this worklog,
    /// such as a delete whose preflight failed, brings the page's recovery
    /// buttons into the row, since the bar can cover the page entirely.
    pub(super) fn render_delete_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.delete_target.as_ref()?;
        let sending = self.delete_in_flight();
        let unconfirmed = self.delete_unconfirmed;
        let recovery = self.target_recovery();
        let discarding = recovery.is_some() && self.discard_confirm;
        let mut lines = vec![if unconfirmed {
            (
                Tone::Warning,
                unconfirmed_delete(target.seconds, target.date),
            )
        } else {
            delete_prompt(target.seconds, target.date)
        }];
        if let Some(error) = &self.error {
            lines.push((Tone::Error, error.clone()));
        }
        if let Some(phase) = recovery {
            lines.push(recovery_line(phase));
            if discarding {
                lines.push(discard_prompt(phase == WritePhase::Ready));
            }
        }
        // The row wraps rather than clip a button in a narrow pane.
        let mut actions = h_flex()
            .id("tempo-delete-actions")
            .test_support()
            .flex_wrap()
            .justify_end()
            .gap_2();
        if discarding {
            actions = actions.children(self.discard_buttons(&BAR_RECOVERY, cx));
        } else if let Some(phase) = recovery {
            // The captured change is retried or discarded, not prepared again.
            actions = actions.children(self.recovery_buttons(phase, &BAR_RECOVERY, cx));
        } else if !unconfirmed {
            actions = actions.child(
                Button::new("tempo-confirm-delete")
                    .label("Confirm deletion")
                    .danger()
                    .disabled(!self.can_submit())
                    .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
            );
        }
        // An unconfirmed delete may already have removed the worklog, so the
        // bar neither offers it again nor claims it is kept.
        let (id, label) = if unconfirmed {
            ("tempo-close-unconfirmed-delete", "Close")
        } else {
            ("tempo-keep-worklog", "Keep worklog")
        };
        actions = actions.child(
            Button::new(id)
                .label(label)
                .disabled(sending)
                .on_click(cx.listener(|this, _, _, cx| this.dismiss_delete(cx))),
        );
        let description = target
            .description
            .as_deref()
            .map(|text| description_line(text, DESCRIPTION_LINE))
            .filter(|line| !line.is_empty())
            .map(|line| {
                let tip = target
                    .description
                    .as_deref()
                    .map(|text| description_tooltip(text, DESCRIPTION_TOOLTIP))
                    .unwrap_or_default();
                let theme = cx.theme();
                div()
                    .id("tempo-delete-description")
                    .aria_label(line.clone())
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(line)
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            });
        let body = div()
            .id("tempo-delete-messages")
            .test_support()
            .min_h_0()
            .max_h(zpx(160.))
            .overflow_y_scroll()
            .child(status_block(lines, Vec::new(), cx));
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
                .min_h_0()
                .overflow_hidden()
                .gap_2()
                .px(zpx(16.))
                .py(zpx(12.))
                .bg(theme.background)
                .border_b_1()
                .border_color(theme.danger)
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .id("tempo-delete-title")
                                .aria_label(title)
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .children(progress)
                        .child(actions),
                )
                .children(description)
                .child(body)
                .into_any_element(),
        )
    }
}

/// The bar's copy of the page's recovery controls.
const BAR_RECOVERY: RecoveryIds = RecoveryIds {
    retry: "tempo-delete-retry-captured-change",
    cleanup: "tempo-delete-recovery-cleanup",
    discard: "tempo-delete-recovery-discard",
    confirm_discard: "tempo-delete-confirm-discard",
    keep_recovery: "tempo-delete-keep-recovery",
};

/// Characters of the description the bar's single line shows.
const DESCRIPTION_LINE: usize = 240;
/// Characters of the description its tooltip shows.
const DESCRIPTION_TOOLTIP: usize = 1000;

/// The description on one line: runs of whitespace, line breaks included,
/// become one space, and text past `limit` characters ends in an ellipsis.
/// Tempo allows 128 KiB, which the bar must not lay out in full. Words are
/// joined until the line passes `limit`, so any text left out, even a word
/// after a line of exactly `limit` characters, leaves the line to `clip`.
pub(super) fn description_line(text: &str, limit: usize) -> String {
    let mut line = String::new();
    for (count, word) in text.split_whitespace().enumerate() {
        if line.chars().count() > limit {
            break;
        }
        if count > 0 {
            line.push(' ');
        }
        line.push_str(word);
    }
    clip(line, limit)
}

/// The tooltip keeps the description's line breaks, up to `limit` characters.
fn description_tooltip(text: &str, limit: usize) -> String {
    clip(text.trim().to_owned(), limit)
}

fn clip(text: String, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text,
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
