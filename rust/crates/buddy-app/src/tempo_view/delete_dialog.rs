//! The worklog deletion confirmation. Timeline's Delete worklog and a grid
//! right-click both set `delete_target`; the prompt opens as a modal dialog,
//! so it is in view wherever the page is scrolled and takes keyboard focus.
use super::render::{delete_prompt, duration, status_block};
use super::*;
use crate::shell::dialogs;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    App, InteractiveElement, IntoElement, ParentElement, Styled, TestSupportExt as _, WeakEntity,
    div,
};

impl TempoView {
    /// Opens or closes the dialog so it matches `delete_target`. Render
    /// defers this call: the dialog lives in the window root, which must not
    /// be updated while this view draws.
    pub(super) fn sync_delete_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.delete_target.is_some();
        if wanted == self.delete_dialog.is_some() {
            return;
        }
        if wanted {
            let view = cx.entity().downgrade();
            let closed = view.clone();
            self.delete_dialog = Some(dialogs::open(
                window,
                cx,
                move |dialog, _, cx| build(dialog, &view, cx),
                move |_, cx| {
                    closed
                        .update(cx, |this, cx| this.delete_dialog_closed(cx))
                        .ok();
                },
            ));
        } else if let Some(token) = self.delete_dialog.take() {
            // Cleared by a write, a refresh or a copy, not by the dialog.
            // Only this dialog closes, even below another such as About.
            dialogs::close(token, window, cx);
        }
    }

    /// The dialog dismissed itself (Escape or the backdrop).
    fn delete_dialog_closed(&mut self, cx: &mut Context<Self>) {
        if self.delete_dialog.take().is_none() {
            return;
        }
        self.delete_target = None;
        cx.notify();
    }

    /// Keep worklog, or Close once the deletion's result is unknown.
    fn dismiss_delete_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_target = None;
        if let Some(token) = self.delete_dialog.take() {
            dialogs::close(token, window, cx);
        }
        cx.notify();
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

/// Built again on every frame, so its buttons follow the view's busy and
/// write state, and a failed delete shows its error here, above the page.
fn build(dialog: Dialog, view: &WeakEntity<TempoView>, cx: &mut App) -> Dialog {
    let Some(entity) = view.upgrade() else {
        return dialog;
    };
    let this = entity.read(cx);
    let Some(target) = &this.delete_target else {
        return dialog;
    };
    let busy = this.busy;
    let can_submit = this.can_submit();
    let unconfirmed = this.delete_unconfirmed;
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
    if let Some(error) = &this.error {
        lines.push((Tone::Error, error.clone()));
    }
    let confirm = view.clone();
    let dismiss = view.clone();
    let dismiss_button = move |id: &'static str, label: &'static str| {
        let dismiss = dismiss.clone();
        Button::new(id)
            .label(label)
            .disabled(busy)
            .on_click(move |_, window, cx| {
                dismiss
                    .update(cx, |this, cx| this.dismiss_delete_dialog(window, cx))
                    .ok();
            })
    };
    // An unconfirmed delete may already have removed the worklog, so the
    // dialog neither offers it again nor claims it is kept.
    let actions = if unconfirmed {
        h_flex()
            .gap_2()
            .child(dismiss_button("tempo-close-unconfirmed-delete", "Close"))
    } else {
        h_flex()
            .gap_2()
            .child(
                Button::new("tempo-confirm-delete")
                    .label("Confirm deletion")
                    .danger()
                    .disabled(!can_submit)
                    .on_click(move |_, _, cx| {
                        confirm.update(cx, |this, cx| this.confirm_delete(cx)).ok();
                    }),
            )
            .child(dismiss_button("tempo-keep-worklog", "Keep worklog"))
    }
    .into_any_element();
    let body = status_block(lines, vec![actions], cx);
    dialog
        .w(gpui_kit::px(480.0))
        .title(if unconfirmed {
            "Deletion unconfirmed"
        } else {
            "Delete worklog"
        })
        .close_button(false)
        // A delete in flight finishes before the prompt can be dismissed.
        .keyboard(!busy)
        .overlay_closable(!busy)
        .child(
            v_flex()
                .id("tempo-delete-dialog")
                .test_support()
                .child(div().child(body)),
        )
}
