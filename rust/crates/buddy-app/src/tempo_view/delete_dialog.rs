//! The worklog deletion confirmation. Timeline's Delete worklog and a grid
//! right-click both set `delete_target`; the prompt opens as a modal dialog,
//! so it is in view wherever the page is scrolled and takes keyboard focus.
use super::render::{delete_prompt, status_block};
use super::*;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
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
        if wanted == self.delete_dialog {
            return;
        }
        self.delete_dialog = wanted;
        if wanted {
            crate::browser_view::Overlays::set(true, cx);
            let view = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, cx| build(dialog, &view, cx));
        } else {
            // Cleared by a write, a refresh or a copy, not by the dialog.
            if window.has_active_dialog(cx) {
                window.close_dialog(cx);
            }
            crate::browser_view::Overlays::set(false, cx);
        }
    }

    /// The dialog has gone (Keep worklog, Escape or the backdrop).
    fn delete_dialog_closed(&mut self, cx: &mut Context<Self>) {
        if !self.delete_dialog {
            return;
        }
        self.delete_dialog = false;
        self.delete_target = None;
        crate::browser_view::Overlays::set(false, cx);
        cx.notify();
    }
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
    let mut lines = vec![delete_prompt(target.seconds, target.date)];
    if let Some(description) = target.description.clone().filter(|text| !text.is_empty()) {
        lines.push((Tone::Info, description));
    }
    if let Some(error) = &this.error {
        lines.push((Tone::Error, error.clone()));
    }
    let confirm = view.clone();
    let keep = view.clone();
    let closed = view.clone();
    let actions = h_flex()
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
        .child(
            Button::new("tempo-keep-worklog")
                .label("Keep worklog")
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    keep.update(cx, |this, cx| this.delete_dialog_closed(cx))
                        .ok();
                }),
        )
        .into_any_element();
    let body = status_block(lines, vec![actions], cx);
    dialog
        .w(gpui_kit::px(480.0))
        .title("Delete worklog")
        .close_button(false)
        // A delete in flight finishes before the prompt can be dismissed.
        .keyboard(!busy)
        .overlay_closable(!busy)
        .on_close(move |_, _, cx| {
            closed
                .update(cx, |this, cx| this.delete_dialog_closed(cx))
                .ok();
        })
        .child(
            v_flex()
                .id("tempo-delete-dialog")
                .test_support()
                .child(div().child(body)),
        )
}
