use super::{PromptEvent, PromptView};
use crate::runtime::Runtime;
use buddy_core::terminal_prompts::{PastePhase, PromptRecovery, PromptWriteStatus};
use buddy_core::terminal_recovery::{JournalKind, JournalWrite, RecoveryStore};
use gpui_kit::Context;
use std::sync::Arc;

impl PromptView {
    pub(super) fn archive_request(&mut self, cx: &mut Context<Self>) {
        if self.write_busy || self.journal_busy || self.unsupported || !cx.has_global::<Runtime>() {
            return;
        }
        let (Some(store), Some(record)) = (self.store.clone(), self.record.as_ref()) else {
            return;
        };
        let Some(id) = record.pending.as_ref().map(|request| request.id()) else {
            return;
        };
        let bytes = match record.bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let Some(sequence) = self.sequence.checked_add(1) else {
            return;
        };
        self.sequence = sequence;
        self.journal_busy = true;
        self.run(cx,async move{tokio::task::spawn_blocking(move|| {
            match store.write(sequence,&bytes)? {
                JournalWrite::Stored|JournalWrite::Unchanged=>store.retain_original(),
                JournalWrite::Superseded=>Err(buddy_core::terminal_recovery::RecoveryError::Sequence),
            }
        }).await},move|this,result,cx| {
            this.journal_busy=false;
            match result {
                Ok(Ok(Some(_)))=>{
                    if let Some(record)=this.record.as_mut().filter(|record|record.pending.as_ref().is_some_and(|request|request.id()==id)) {
                        record.pending=None;this.write_ready=None;
                        this.status=Some("Captured request archived locally. The earlier write may have applied. Compare the current saved prompts before saving again.".into());
                        this.queue(cx);this.load(cx);
                    }
                }
                Ok(Ok(None))=>this.error=Some("The captured request could not be retained. It remains locked.".into()),
                Ok(Err(error))=>this.error=Some(error.to_string()),
                Err(_)=>this.error=Some("The captured-request archive worker did not finish.".into()),
            }
        });
    }

    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading || !cx.has_global::<Runtime>() {
            return;
        }
        if self.journal_busy || self.write_busy {
            self.reload_after_journal = true;
            return;
        }
        self.reload_after_journal = false;
        let Some(backend) = self.backend.clone() else {
            self.error = Some("The prompt service address is invalid.".into());
            return;
        };
        self.loading = true;
        let deployment = backend.deployment().to_string();
        self.run(
            cx,
            async move {
                let local = tokio::task::spawn_blocking(move || {
                    let mut restored = None;
                    let path = buddy_core::config::config_path()
                        .map_err(|_| "The prompt recovery location is unavailable.".to_string())?;
                    let (store, snapshot) =
                        RecoveryStore::open(&path, &deployment, JournalKind::Prompts, |bytes| {
                            match PromptRecovery::restore(&deployment, bytes) {
                                Ok(record) => {
                                    restored = Some(record);
                                    true
                                }
                                Err(_) => false,
                            }
                        })
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>((Arc::new(store), snapshot.needs_recovery, restored))
                });
                let (local, remote) = tokio::join!(local, backend.load_prompts());
                (local, remote)
            },
            |this, (local, remote), cx| {
                this.loading = false;
                this.finish_recovery_load(
                    local.unwrap_or_else(|_| {
                        Err("The prompt recovery reader did not finish.".into())
                    }),
                );
                match remote {
                    Ok(prompts) => this.prompts = prompts,
                    Err(error) => this.error = Some(error.to_string()),
                }
                // Loading is read-only. Neither recovered writes nor insertions run here.
                if this.sequence > this.durable {
                    this.flush(cx);
                }
                cx.notify();
            },
        );
    }

    pub(super) fn finish_recovery_load(
        &mut self,
        local: Result<(Arc<RecoveryStore>, bool, Option<PromptRecovery>), String>,
    ) {
        if self.initialized {
            match local {
                Ok((store, unsupported, _)) => {
                    let changed = self
                        .store
                        .as_ref()
                        .is_some_and(|old| !old.has_same_snapshot(&store));
                    self.store = Some(store);
                    self.unsupported |= changed || unsupported;
                    self.error = None;
                    self.status = Some(if self.unsupported {
                        "Local prompt recovery changed or needs attention. Your current draft is retained. Retain the original before continuing."
                    } else { "Prompt list refreshed. Your draft is retained." }.into());
                }
                Err(error) => self.error = Some(error),
            }
            return;
        }
        self.initialized = false;
        match local {
            Ok((store, unsupported, record)) => {
                self.store = Some(store);
                self.unsupported = unsupported;
                self.record = record.or_else(|| {
                    self.backend
                        .as_ref()
                        .and_then(|backend| PromptRecovery::new(backend.deployment()).ok())
                });
                self.initialized = self.record.is_some();
                self.error = None;
                self.populate = true;
                if unsupported {
                    self.error=Some("The existing prompt recovery format needs attention. Retain its original before replacing it.".into());
                } else if self
                    .record
                    .as_ref()
                    .is_some_and(|record| record.pending.is_some() || record.paste.is_some())
                {
                    self.status=Some("A prompt action was recovered. Retry the saved write explicitly; terminal insertion is never replayed.".into());
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn flush_on_quit(&mut self) {
        let (Some(store), Some(record)) = (&self.store, &self.record) else {
            return;
        };
        let result = record
            .bytes()
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                let sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| "Prompt recovery sequence exhausted.".to_string())?;
                store
                    .flush_on_quit(sequence, &bytes)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(Some(path)) => log::warn!(
                "Latest prompt recovery retained separately at {}",
                path.display()
            ),
            Ok(None) => {}
            Err(error) => log::error!("Could not flush prompt recovery before quitting: {error}"),
        }
    }

    pub(super) fn queue(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.sequence.checked_add(1) else {
            self.error = Some("Prompt recovery sequencing is exhausted.".into());
            return;
        };
        self.sequence = next;
        self.flush(cx);
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.journal_busy || self.loading || self.unsupported || !cx.has_global::<Runtime>() {
            return;
        }
        let (Some(store), Some(record)) = (self.store.clone(), self.record.as_ref()) else {
            return;
        };
        let bytes = match record.bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let sequence = self.sequence;
        let write = record.pending.as_ref().map(|request| request.id());
        let paste = record.paste.as_ref().map(|paste| (paste.id, paste.phase));
        self.journal_busy = true;
        self.run(
            cx,
            async move { tokio::task::spawn_blocking(move || store.write(sequence, &bytes)).await },
            move |this, result, cx| {
                this.journal_busy = false;
                match result {
                    Ok(Ok(JournalWrite::Stored | JournalWrite::Unchanged)) => {
                        this.durable = sequence;
                        if let Some(id) = write.filter(|id| this.write_ready == Some(*id)) {
                            this.send_write(id, cx);
                        }
                        if let Some((id, phase)) = paste.filter(|(id, phase)| {
                            this.record
                                .as_ref()
                                .and_then(|r| r.paste.as_ref())
                                .is_some_and(|p| p.id == *id && p.phase == *phase)
                        }) {
                            match phase {
                                PastePhase::Prepared => this.send_paste(id, cx),
                                PastePhase::Written => this.send_metadata(id, cx),
                                PastePhase::Uncertain => {}
                            }
                        }
                    }
                    Ok(Ok(JournalWrite::Superseded)) => {}
                    Ok(Err(error)) => {
                        this.error = Some(error.to_string());
                        return;
                    }
                    Err(_) => {
                        this.error = Some("The prompt recovery writer did not finish.".into());
                        return;
                    }
                }
                if this.sequence > sequence {
                    this.flush(cx);
                } else if this.reload_after_journal {
                    this.load(cx);
                }
            },
        );
    }

    fn send_write(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        if self.write_busy {
            return;
        }
        let Some(request) = self
            .record
            .as_ref()
            .and_then(|record| record.pending.clone())
            .filter(|request| request.id() == id)
        else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        self.write_ready = None;
        self.write_busy = true;
        self.error = None;
        self.run(
            cx,
            async move { backend.write_prompt(&request).await },
            move |this, result, cx| this.finish_write(id, result, cx),
        );
    }

    pub(super) fn finish_write(
        &mut self,
        id: uuid::Uuid,
        result: Result<
            buddy_core::terminal_prompts::PromptReceipt,
            buddy_core::terminal_backend::BackendError,
        >,
        cx: &mut Context<Self>,
    ) {
        self.write_busy = false;
        let Some(record) = self.record.as_mut().filter(|record| {
            record
                .pending
                .as_ref()
                .is_some_and(|request| request.id() == id)
        }) else {
            return;
        };
        match result {
            Ok(receipt) => {
                record.pending = None;
                if receipt.status != PromptWriteStatus::Conflict {
                    record.draft = None;
                    self.status =
                        Some("Prompt write acknowledged. Reloading the current list…".into());
                } else {
                    self.error=Some("The prompt changed elsewhere. Your draft is retained. Reload and compare before editing again.".into());
                }
                self.delete_confirm = false;
                self.queue(cx);
                self.load(cx);
            }
            Err(error) => {
                self.error = Some(format!(
                    "{error} The immutable request is retained. Retry it explicitly."
                ));
                self.queue(cx);
            }
        }
    }

    fn send_paste(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        let Some((target, text)) = self.paste_ready.take() else {
            return;
        };
        if self.visible && self.target.as_ref() == Some(&target) {
            cx.emit(PromptEvent::Paste { id, target, text });
        } else {
            self.paste_result(&id.to_string(), Some(false), cx);
        }
    }

    fn send_metadata(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        if self.metadata_busy {
            return;
        }
        let Some(prompt) = self
            .record
            .as_ref()
            .and_then(|record| record.paste.as_ref())
            .filter(|paste| paste.id == id && paste.phase == PastePhase::Written)
            .map(|paste| paste.prompt.clone())
        else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        self.metadata_busy = true;
        self.run(cx,async move{backend.mark_prompt_used(&prompt).await},move|this,result,cx| {
            this.metadata_busy=false;
            if this.record.as_ref().and_then(|record|record.paste.as_ref()).is_none_or(|paste|paste.id!=id){return;}
            match result {
                Ok(())=>{this.record.as_mut().expect("record").paste=None;this.status=Some("Prompt inserted. No Enter key was added.".into());this.queue(cx);this.load(cx);}
                Err(error)=>this.error=Some(format!("Prompt was written, but its last-used time could not be saved. {error} Retry metadata only; do not insert again.")),
            }
        });
    }

    pub(super) fn retain_original(&mut self, cx: &mut Context<Self>) {
        if self.journal_busy || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.journal_busy = true;
        self.run(
            cx,
            async move { tokio::task::spawn_blocking(move || store.retain_original()).await },
            |this, result, cx| {
                this.journal_busy = false;
                match result {
                    Ok(Ok(_)) => {
                        this.unsupported = false;
                        this.error = None;
                        this.queue(cx);
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    Err(_) => {
                        this.error =
                            Some("The original prompt recovery file could not be retained.".into())
                    }
                }
            },
        );
    }
}
