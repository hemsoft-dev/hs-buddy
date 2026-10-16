//! The recovery write completes before a backend mutation is allowed to start.
use std::sync::Arc;

use crate::runtime::Runtime;
use buddy_core::terminal_backend::WorkspaceSnapshot;
use buddy_core::terminal_recovery::{JournalKind, JournalWrite, RecoveryStore};
use buddy_core::terminal_workspaces::{
    LayoutError, SaveStatus, WorkspaceDocument, WorkspaceEditor,
};
use gpui_kit::Context;

use super::TerminalView;

struct LocalLoad {
    store: Option<Arc<RecoveryStore>>,
    editor: Option<WorkspaceEditor>,
    unsupported: bool,
    error: Option<String>,
}

#[cfg(test)]
mod tests;

fn local_load(deployment: &str) -> LocalLoad {
    let mut restored = None;
    let open = buddy_core::config::config_path()
        .map_err(|error| error.to_string())
        .and_then(|path| {
            RecoveryStore::open(&path, deployment, JournalKind::Workspaces, |bytes| {
                match WorkspaceEditor::restore_journal(deployment, Arc::from(bytes)) {
                    Ok(editor) => {
                        restored = Some(editor);
                        true
                    }
                    Err(_) => false,
                }
            })
            .map_err(|error| error.to_string())
        });
    match open {
        Ok((store, snapshot)) => LocalLoad {
            store: Some(Arc::new(store)),
            editor: restored,
            unsupported: snapshot.needs_recovery,
            error: None,
        },
        Err(error) => LocalLoad {
            store: None,
            editor: None,
            unsupported: false,
            error: Some(error),
        },
    }
}

impl TerminalView {
    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.error = Some("The configured workspace service address is invalid.".into());
            return;
        };
        self.loading = true;
        let ticket = uuid::Uuid::new_v4();
        self.load_ticket = ticket;
        if self.initialized {
            self.run(cx, async move { backend.load_workspace().await }, move |this, result, _| {
                if this.load_ticket != ticket { return; }
                this.loading = false;
                match result {
                    Ok(snapshot) => { this.remote = Some(snapshot); this.status = Some("Compare the saved layout with your local workspace before choosing a version.".into()); }
                    Err(error) => this.error = Some(error.to_string()),
                }
            });
            return;
        }
        self.status = Some("Loading saved workspaces and local recovery…".into());
        let deployment = backend.deployment().to_string();
        self.run(
            cx,
            async move {
                let local = tokio::task::spawn_blocking(move || local_load(&deployment));
                let (local, remote) = tokio::join!(local, backend.load_workspace());
                let local = local.unwrap_or_else(|_| LocalLoad {
                    store: None,
                    editor: None,
                    unsupported: false,
                    error: Some("The local recovery reader did not finish.".into()),
                });
                (local, remote)
            },
            move |this, (local, remote), cx| {
                if this.load_ticket != ticket {
                    return;
                }
                this.finish_initial_load(local, remote, cx);
            },
        );
    }

    fn finish_initial_load(
        &mut self,
        local: LocalLoad,
        remote: Result<WorkspaceSnapshot, buddy_core::terminal_backend::BackendError>,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        self.initialized = local.store.is_some();
        self.store = local.store;
        self.unsupported_recovery = local.unsupported;
        self.error = local.error;
        if !self.initialized {
            self.status = Some(
                "Local recovery is unavailable. Reload to retry before editing workspaces.".into(),
            );
            return;
        }
        let backend = self.backend.as_ref().expect("bound backend");
        let deployment = backend.deployment().to_string();
        let snapshot = match remote {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                self.error = Some(error.to_string());
                None
            }
        };
        if self.unsupported_recovery {
            self.remote = snapshot;
            self.status = Some(
                "A local recovery file needs attention. Retain its original before replacing it."
                    .into(),
            );
            return;
        }
        match (local.editor, snapshot) {
            (Some(editor), Some(snapshot)) if editor.status() != SaveStatus::Clean => {
                self.editor = Some(editor);
                self.remote = Some(snapshot);
                self.backend_checked = false;
                self.status = Some("A local draft was recovered. Choose how to reconcile it with the saved layout.".into());
            }
            (_, Some(snapshot)) => {
                self.editor = WorkspaceEditor::new(&deployment, snapshot).ok();
                self.backend_checked = true;
                self.status = Some("Saved workspace loaded.".into());
            }
            (Some(editor), None) => {
                self.editor = Some(editor);
                self.backend_checked = false;
                self.status = Some(
                    "Local workspace recovered. Reload the service before saving shared changes."
                        .into(),
                );
            }
            (None, None) => {
                let document =
                    WorkspaceDocument::restore(Arc::from(&b"null"[..])).expect("empty workspace");
                self.editor = WorkspaceEditor::new(
                    &deployment,
                    WorkspaceSnapshot {
                        document,
                        revision: 0,
                    },
                )
                .ok();
                self.backend_checked = false;
                self.status = Some(
                    "Work locally. Reload and compare before saving to the workspace service."
                        .into(),
                );
            }
        }
        self.populate_fields = true;
        self.synchronize_sessions(cx);
        self.queue_journal(cx);
    }

    pub(super) fn edit_workspace(
        &mut self,
        edit: impl FnOnce(&WorkspaceDocument) -> Result<WorkspaceDocument, LayoutError>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.editor.as_mut() else {
            return false;
        };
        let changed = match edit(editor.document()).and_then(|document| {
            editor.edit(document).map_err(|_| LayoutError {
                path: "$".into(),
                reason: "workspace edit could not be retained",
            })
        }) {
            Ok(()) => {
                self.error = None;
                self.synchronize_sessions(cx);
                self.prepare_save(cx);
                self.queue_journal(cx);
                true
            }
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        };
        cx.notify();
        changed
    }

    pub(super) fn prepare_save(&mut self, cx: &mut Context<Self>) {
        if !self.backend_checked || self.store.is_none() || self.unsupported_recovery {
            return;
        }
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.status() == SaveStatus::Dirty {
            match editor.begin_save() {
                Ok(request) => {
                    self.pending_send = Some(request);
                    self.status = Some("Retaining the save request locally…".into());
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        cx.notify();
    }

    pub(super) fn save_now(&mut self, cx: &mut Context<Self>) {
        if self.pending_send.is_some() {
            self.queue_journal(cx);
            return;
        }
        if !self.backend_checked {
            self.status =
                Some("Reload and compare the saved layout before saving shared changes.".into());
            self.load(cx);
            return;
        }
        if let Some(editor) = self.editor.as_mut() {
            let request = match editor.status() {
                SaveStatus::Uncertain => editor.retry_uncertain(),
                SaveStatus::Dirty | SaveStatus::Rejected => editor.begin_save(),
                _ => return,
            };
            match request {
                Ok(request) => {
                    self.pending_send = Some(request);
                    self.queue_journal(cx);
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }

    pub(super) fn queue_journal(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.journal_sequence.checked_add(1) else {
            self.error = Some(
                "Local recovery sequencing is exhausted. Copy this workspace before restarting."
                    .into(),
            );
            return;
        };
        self.journal_sequence = next;
        self.flush_journal(cx);
    }

    fn flush_journal(&mut self, cx: &mut Context<Self>) {
        if self.journal_busy
            || self.unsupported_recovery
            || self.editor.is_none()
            || !cx.has_global::<Runtime>()
        {
            return;
        }
        let Some(store) = self.store.clone() else {
            self.status = Some("Local recovery is unavailable. Shared saves are paused; your current workspace remains open.".into());
            return;
        };
        let bytes = match self.editor.as_ref().expect("editor").journal() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let sequence = self.journal_sequence;
        let request = self
            .pending_send
            .as_ref()
            .map(|request| request.id().to_string());
        self.journal_busy = true;
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || store.write(sequence, &bytes))
                    .await
                    .map_err(|_| "The local recovery writer did not finish.".to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()))
            },
            move |this, result, cx| {
                this.journal_busy = false;
                match result {
                    Ok(JournalWrite::Stored | JournalWrite::Unchanged) => {
                        this.journal_durable = sequence;
                        if request.as_deref().is_some_and(|id| {
                            this.pending_send
                                .as_ref()
                                .is_some_and(|pending| pending.id() == id)
                        }) {
                            this.send_prepared(cx);
                        }
                    }
                    Ok(JournalWrite::Superseded) => {}
                    Err(error) => {
                        this.error = Some(error);
                        return;
                    }
                }
                if this.journal_sequence > sequence {
                    this.flush_journal(cx);
                }
            },
        );
    }

    fn send_prepared(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending_send.take() else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = request.id().to_string();
        self.status = Some("Saving the retained workspace request…".into());
        self.run(cx, async move { backend.save_workspace(request.arguments(), request.expected_revision()).await },
            move |this, result, cx| {
                let error = result.as_ref().err().map(ToString::to_string);
                let Some(editor) = this.editor.as_mut() else { return; };
                match editor.complete(&id, result) {
                    Ok(true) => {
                        this.error = error;
                        this.status = Some(match editor.status() {
                            SaveStatus::Clean => "Workspace saved.",
                            SaveStatus::Dirty => "Saved; retaining newer local edits…",
                            SaveStatus::Uncertain => "Save result unknown. Retry the same request or reload to compare.",
                            SaveStatus::Conflict { .. } => "The saved layout changed elsewhere. Reload to compare; your local layout is retained.",
                            _ => "The save was rejected. Your local layout is retained.",
                        }.into());
                        this.prepare_save(cx); this.queue_journal(cx);
                    }
                    Ok(false) => {},
                    Err(error) => { this.error = Some(error.to_string()); this.queue_journal(cx); }
                }
            });
    }

    pub(super) fn keep_local(&mut self, cx: &mut Context<Self>) {
        let (Some(snapshot), Some(editor)) = (self.remote.as_ref(), self.editor.as_mut()) else {
            return;
        };
        match editor.keep_local_after_reload(snapshot) {
            Ok(()) => {
                self.pending_send = None;
                self.backend_checked = true;
                self.remote = None;
                self.prepare_save(cx);
                self.queue_journal(cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(super) fn use_remote(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.remote.take() else {
            return;
        };
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        self.pending_send = None;
        self.backend_checked = true;
        if let Some(editor) = self.editor.as_mut() {
            if let Err(error) = editor.use_remote_after_reload(snapshot) {
                self.error = Some(error.to_string());
                return;
            }
        } else {
            self.editor = WorkspaceEditor::new(backend.deployment(), snapshot).ok();
        }
        self.populate_fields = true;
        self.synchronize_sessions(cx);
        self.queue_journal(cx);
    }

    pub(super) fn retain_and_use_remote(&mut self, cx: &mut Context<Self>) {
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
                        this.unsupported_recovery = false;
                        if this.remote.is_some() {
                            this.use_remote(cx);
                        } else if let Some(backend) = &this.backend {
                            let document = WorkspaceDocument::restore(Arc::from(&b"null"[..]))
                                .expect("empty workspace");
                            this.editor = WorkspaceEditor::new(
                                backend.deployment(),
                                WorkspaceSnapshot {
                                    document,
                                    revision: 0,
                                },
                            )
                            .ok();
                            this.backend_checked = false;
                            this.queue_journal(cx);
                        }
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    Err(_) => {
                        this.error =
                            Some("The original recovery file could not be retained.".into())
                    }
                }
            },
        );
    }
}
