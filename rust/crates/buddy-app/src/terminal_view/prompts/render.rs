use super::PromptView;
use crate::zoom::zpx;
use buddy_core::terminal_prompts::PastePhase;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div,
};

impl PromptView {
    fn draft_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = self
            .record
            .as_ref()
            .is_some_and(|record| record.pending.is_some());
        let mut panel = v_flex()
            .gap_2()
            .child("Prompt name")
            .child(Input::new(&self.title).disabled(pending))
            .child("Prompt text")
            .child(
                div()
                    .h(zpx(140.0))
                    .child(Textarea::new(&self.content).disabled(pending)),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("terminal-prompt-save")
                            .label("Save prompt")
                            .disabled(pending || self.unsupported || self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.save(false, cx))),
                    )
                    .child(
                        Button::new("terminal-prompt-discard")
                            .label("Discard draft")
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(record) = &mut this.record {
                                    record.draft = None;
                                }
                                this.delete_confirm = false;
                                this.queue(cx);
                                cx.notify();
                            })),
                    ),
            );
        if self
            .record
            .as_ref()
            .and_then(|record| record.draft.as_ref())
            .is_some_and(|draft| draft.original.is_some())
        {
            panel = panel.child(
                Button::new("terminal-prompt-delete")
                    .label("Delete prompt…")
                    .disabled(pending || self.unsupported)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.delete_confirm = true;
                        cx.notify();
                    })),
            );
        }
        if self.delete_confirm {
            panel = panel.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child("Delete this saved prompt?")
                    .child(
                        Button::new("terminal-prompt-confirm-delete")
                            .label("Delete saved prompt")
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| this.save(true, cx))),
                    )
                    .child(
                        Button::new("terminal-prompt-cancel-delete")
                            .label("Keep prompt")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delete_confirm = false;
                                cx.notify();
                            })),
                    ),
            );
        }
        panel.into_any_element()
    }

    fn receipt_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut controls = v_flex().gap_2();
        if self
            .record
            .as_ref()
            .is_some_and(|record| record.pending.is_some())
        {
            controls=controls.child("A captured write is retained. Its fields stay fixed until the service acknowledges it.")
                .child(Button::new("terminal-prompt-retry-write").label("Retry saved prompt write").disabled(self.write_busy||self.journal_busy||self.unsupported).on_click(cx.listener(|this,_,_,cx| {
                    this.write_ready=this.record.as_ref().and_then(|record|record.pending.as_ref()).map(|request|request.id());this.queue(cx);
                })))
                .child(Button::new("terminal-prompt-archive-request").label("Archive captured request and compare").disabled(self.write_busy||self.journal_busy||self.unsupported).on_click(cx.listener(|this,_,_,cx|this.archive_request(cx))));
        }
        if let Some(paste) = self
            .record
            .as_ref()
            .and_then(|record| record.paste.as_ref())
        {
            match paste.phase {
                PastePhase::Written=>controls=controls.child("Prompt already written. Retrying its last-used time does not paste again.")
                    .child(Button::new("terminal-prompt-retry-metadata").label("Retry last-used time only").disabled(self.metadata_busy||self.journal_busy||self.unsupported).on_click(cx.listener(|this,_,_,cx|this.queue(cx)))),
                PastePhase::Uncertain=>controls=controls.child("Check the terminal before using this prompt again. An uncertain insertion is never repeated automatically.")
                    .child(Button::new("terminal-prompt-clear-insertion").label("I've checked the shell; clear insertion receipt").disabled(self.journal_busy||self.unsupported).on_click(cx.listener(|this,_,_,cx| {
                        if let Some(record)=&mut this.record {record.paste=None;}this.queue(cx);
                    }))),
                PastePhase::Prepared=>controls=controls.child("Preparing insertion or awaiting the shell write acknowledgement…"),
            }
        }
        controls.into_any_element()
    }
}

impl Render for PromptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.populate {
            self.populate = false;
            if let Some(draft) = self
                .record
                .as_ref()
                .and_then(|record| record.draft.as_ref())
            {
                let title = draft.title.clone();
                let content = draft.content.clone();
                self.title
                    .update(cx, |input, cx| input.set_value(title, window, cx));
                self.content
                    .update(cx, |input, cx| input.set_value(content, window, cx));
            }
        }
        let blocked = self.unsupported || self.record.is_none();
        let editing = self
            .record
            .as_ref()
            .is_some_and(|record| record.draft.is_some());
        let mut body = v_flex().gap_2().p_3().child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("Prompt library"),
                )
                .child(
                    Button::new("terminal-prompt-new")
                        .label("New prompt")
                        .disabled(blocked || editing)
                        .on_click(cx.listener(|this, _, _, cx| this.edit(None, cx))),
                )
                .child(
                    Button::new("terminal-prompts-reload")
                        .label("Reload prompts")
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                ),
        );
        if let Some(status) = &self.status {
            body = body.child(div().child(status.clone()));
        }
        if let Some(error) = &self.error {
            body = body.child(div().text_color(cx.theme().danger).child(error.clone()));
        }
        if self.unsupported {
            body = body.child(
                Button::new("terminal-prompts-retain")
                    .label("Retain original prompt recovery and continue")
                    .disabled(self.journal_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.retain_original(cx))),
            );
        }
        body = body.child(self.receipt_controls(cx));
        if editing {
            body = body.child(self.draft_controls(cx));
            let mut saved = v_flex()
                .gap_2()
                .child("Current saved prompts. Reload to refresh this comparison.");
            for prompt in &self.prompts {
                saved = saved.child(
                    v_flex()
                        .gap_1()
                        .child(prompt.title.clone())
                        .child(div().whitespace_normal().child(prompt.content.clone())),
                );
            }
            body = body.child(div().max_h(zpx(180.0)).overflow_y_scrollbar().child(saved));
        } else {
            let can_insert = !blocked
                && self.target.is_some()
                && self
                    .record
                    .as_ref()
                    .is_some_and(|record| record.paste.is_none());
            for (index, prompt) in self.prompts.iter().enumerate() {
                let edit = prompt.clone();
                let insert = prompt.clone();
                let copy = prompt.content.clone();
                body = body.child(
                    v_flex()
                        .gap_2()
                        .child(div().whitespace_normal().child(prompt.title.clone()))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new(format!("terminal-prompt-insert-{index}"))
                                        .label("Insert + copy")
                                        .disabled(!can_insert)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.insert(insert.clone(), cx)
                                        })),
                                )
                                .child(
                                    Button::new(format!("terminal-prompt-copy-{index}"))
                                        .label("Copy")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ))
                                        }),
                                )
                                .child(
                                    Button::new(format!("terminal-prompt-edit-{index}"))
                                        .label("Edit prompt")
                                        .disabled(blocked)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit(Some(edit.clone()), cx)
                                        })),
                                ),
                        ),
                );
            }
            if self.prompts.is_empty() && !self.loading {
                body = body.child("No prompts loaded. Create a prompt or reload the service.");
            }
        }
        div()
            .id("terminal-prompt-library")
            .h_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(body)
    }
}
