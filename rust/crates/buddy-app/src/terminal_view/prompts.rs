//! Prompt editing is recoverable; terminal insertion has a separate, never replayed receipt.
mod io;
mod render;
#[cfg(test)]
mod tests;

use crate::runtime::Runtime;
use buddy_core::terminal_backend::TerminalBackend;
use buddy_core::terminal_prompts::{
    PasteIntent, PastePhase, Prompt, PromptDraft, PromptRecovery, PromptRequest,
};
use buddy_core::terminal_recovery::RecoveryStore;
use buddy_core::terminal_workspaces::LaunchTicket;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, Window};
use std::{future::Future, sync::Arc};

pub(super) enum PromptEvent {
    Paste {
        id: uuid::Uuid,
        target: LaunchTicket,
        text: String,
    },
}

pub(super) struct PromptView {
    backend: Option<TerminalBackend>,
    record: Option<PromptRecovery>,
    store: Option<Arc<RecoveryStore>>,
    unsupported: bool,
    prompts: Vec<Prompt>,
    initialized: bool,
    visible: bool,
    loading: bool,
    reload_after_journal: bool,
    target: Option<LaunchTicket>,
    title: Entity<InputState>,
    content: Entity<TextareaState>,
    populate: bool,
    sequence: u64,
    durable: u64,
    journal_busy: bool,
    write_ready: Option<uuid::Uuid>,
    write_busy: bool,
    paste_ready: Option<(LaunchTicket, String)>,
    metadata_busy: bool,
    delete_confirm: bool,
    status: Option<String>,
    error: Option<String>,
}
impl EventEmitter<PromptEvent> for PromptView {}

impl PromptView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Build project"));
        let content = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Prompt text. Inserting never adds Enter.")
        });
        cx.on_app_quit(|this, _| {
            this.flush_on_quit();
            std::future::ready(())
        })
        .detach();
        cx.subscribe(&title, |this, _, _: &InputEvent, cx| this.retain_draft(cx))
            .detach();
        cx.subscribe(&content, |this, _, _: &InputEvent, cx| {
            this.retain_draft(cx)
        })
        .detach();
        Self {
            backend: TerminalBackend::new(&buddy_core::convex_data::convex_url()).ok(),
            record: None,
            store: None,
            unsupported: false,
            prompts: Vec::new(),
            initialized: false,
            visible: false,
            loading: false,
            reload_after_journal: false,
            target: None,
            title,
            content,
            populate: false,
            sequence: 0,
            durable: 0,
            journal_busy: false,
            write_ready: None,
            write_busy: false,
            paste_ready: None,
            metadata_busy: false,
            delete_confirm: false,
            status: None,
            error: None,
        }
    }

    pub fn set_target(
        &mut self,
        visible: bool,
        target: Option<LaunchTicket>,
        cx: &mut Context<Self>,
    ) {
        if (!visible || self.target != target) && self.paste_ready.take().is_some() {
            if let Some(record) = &mut self.record {
                record.paste = None;
            }
            self.status = Some("The insertion target changed before the paste was sent.".into());
            self.queue(cx);
        } else if (!visible || self.target != target)
            && let Some(paste) = self
                .record
                .as_mut()
                .and_then(|record| record.paste.as_mut())
            && paste.phase == PastePhase::Prepared
        {
            paste.phase = PastePhase::Uncertain;
            self.status = Some(
                "The insertion target changed. Check the shell before inserting again.".into(),
            );
            self.queue(cx);
        }
        self.visible = visible;
        self.target = target;
        if visible && !self.initialized && !self.loading {
            self.load(cx);
        }
        cx.notify();
    }

    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let receiver = Runtime::global(cx).spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = receiver.await {
                this.update(cx, |this, cx| {
                    apply(this, result, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn retain_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(record) = &mut self.record
            && record.pending.is_none()
            && let Some(draft) = &mut record.draft
        {
            draft.title = self.title.read(cx).value().to_string();
            draft.content = self.content.read(cx).value().to_string();
            self.queue(cx);
        }
        cx.notify();
    }

    fn edit(&mut self, prompt: Option<Prompt>, cx: &mut Context<Self>) {
        let Some(record) = &mut self.record else {
            return;
        };
        if record.pending.is_some() || record.draft.is_some() {
            self.status = Some(
                "Save or explicitly discard the current draft before opening another prompt."
                    .into(),
            );
            return;
        }
        record.draft = Some(PromptDraft {
            title: prompt.as_ref().map_or(String::new(), |p| p.title.clone()),
            content: prompt.as_ref().map_or(String::new(), |p| p.content.clone()),
            original: prompt,
        });
        self.populate = true;
        self.delete_confirm = false;
        self.queue(cx);
    }

    fn save(&mut self, remove: bool, cx: &mut Context<Self>) {
        self.retain_draft(cx);
        let Some(record) = &mut self.record else {
            return;
        };
        if record.pending.is_some() || self.write_busy {
            return;
        }
        let Some(draft) = &record.draft else { return };
        let request = if remove {
            draft
                .original
                .as_ref()
                .ok_or(buddy_core::terminal_backend::BackendError::Rejected)
                .and_then(PromptRequest::remove)
        } else if let Some(original) = &draft.original {
            PromptRequest::update(
                original,
                draft.title.trim().into(),
                draft.content.replace("\r\n", "\n"),
            )
        } else {
            PromptRequest::create(
                draft.title.trim().into(),
                draft.content.replace("\r\n", "\n"),
            )
        };
        match request {
            Ok(request) => {
                self.write_ready = Some(request.id());
                record.pending = Some(request);
                self.queue(cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn insert(&mut self, prompt: Prompt, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone().filter(|_| self.visible) else {
            return;
        };
        let Some(record) = &mut self.record else {
            return;
        };
        if record.paste.is_some() {
            return;
        }
        record.paste = Some(PasteIntent {
            id: uuid::Uuid::new_v4(),
            prompt: prompt.id,
            pane: target.pane().into(),
            generation: target.generation(),
            phase: PastePhase::Prepared,
        });
        self.paste_ready = Some((target, prompt.content));
        self.status = Some("Retaining the insertion receipt before sending…".into());
        self.queue(cx);
    }

    pub fn paste_result(&mut self, tag: &str, written: Option<bool>, cx: &mut Context<Self>) {
        let Some(record) = &mut self.record else {
            return;
        };
        let Some(paste) = &mut record.paste else {
            return;
        };
        if paste.id.to_string() != tag {
            return;
        }
        match written {
            Some(true) => {
                paste.phase = PastePhase::Written;
                self.status = Some("Prompt written. Updating its last-used time…".into());
            }
            Some(false) => {
                record.paste = None;
                self.status = Some("Prompt was not sent.".into());
            }
            None => {
                paste.phase = PastePhase::Uncertain;
                self.error = Some(
                    "Prompt insertion is uncertain. Inspect the shell before inserting again."
                        .into(),
                );
            }
        }
        self.queue(cx);
    }
}
