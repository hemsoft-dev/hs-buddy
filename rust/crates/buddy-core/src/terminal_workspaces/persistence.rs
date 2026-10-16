//! Serialize native saves, retain edits behind an immutable request, and recover after restart.
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::WorkspaceDocument;
use crate::terminal_backend::{BackendError, SaveReceipt, TerminalBackend, WorkspaceSnapshot};

const MAX_REVISION: u64 = 9_007_199_254_740_991;
const MAX_JOURNAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStatus {
    Clean,
    Dirty,
    Saving,
    Uncertain,
    Rejected,
    Conflict { remote_revision: u64 },
}

#[derive(Clone)]
pub struct SaveRequest {
    id: String,
    expected_revision: u64,
    generation: u64,
    document: WorkspaceDocument,
}

impl std::fmt::Debug for SaveRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveRequest")
            .field("id", &self.id)
            .field("expected_revision", &self.expected_revision)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl SaveRequest {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    pub fn arguments(&self) -> Value {
        let mut arguments = self.document.save_arguments();
        arguments["requestId"] = self.id.clone().into();
        arguments["expectedRevision"] = self.expected_revision.into();
        arguments
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EditorError(pub &'static str);
impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for EditorError {}

pub struct JournalFailure {
    pub original: Arc<[u8]>,
    pub error: EditorError,
}
impl std::fmt::Debug for JournalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

pub struct WorkspaceEditor {
    deployment: String,
    document: WorkspaceDocument,
    base_revision: u64,
    generation: u64,
    pending: Option<SaveRequest>,
    status: SaveStatus,
}

impl std::fmt::Debug for WorkspaceEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceEditor")
            .field("revision", &self.base_revision)
            .field("generation", &self.generation)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl WorkspaceEditor {
    pub fn new(deployment: &str, snapshot: WorkspaceSnapshot) -> Result<Self, EditorError> {
        let deployment = TerminalBackend::new(deployment)
            .map_err(|_| EditorError("Invalid workspace deployment."))?
            .deployment()
            .into();
        if snapshot.revision > MAX_REVISION {
            return Err(EditorError("Invalid workspace revision."));
        }
        Ok(Self {
            deployment,
            document: snapshot.document,
            base_revision: snapshot.revision,
            generation: 0,
            pending: None,
            status: SaveStatus::Clean,
        })
    }

    pub fn document(&self) -> &WorkspaceDocument {
        &self.document
    }
    pub fn status(&self) -> SaveStatus {
        self.status
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn revision(&self) -> u64 {
        self.base_revision
    }

    pub fn edit(&mut self, document: WorkspaceDocument) -> Result<(), EditorError> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or(EditorError("Workspace edit generation is exhausted."))?;
        self.document = document;
        self.generation = next;
        if matches!(self.status, SaveStatus::Clean | SaveStatus::Dirty) {
            self.status = SaveStatus::Dirty;
        }
        Ok(())
    }

    pub fn begin_save(&mut self) -> Result<SaveRequest, EditorError> {
        if self.pending.is_some()
            || !matches!(self.status, SaveStatus::Dirty | SaveStatus::Rejected)
        {
            return Err(EditorError(
                "A workspace save is already pending or needs reconciliation.",
            ));
        }
        if self.base_revision >= MAX_REVISION {
            return Err(EditorError(
                "Workspace revision is exhausted. Keep or export this draft.",
            ));
        }
        let request = SaveRequest {
            id: uuid::Uuid::new_v4().to_string(),
            expected_revision: self.base_revision,
            generation: self.generation,
            document: self.document.clone(),
        };
        self.pending = Some(request.clone());
        self.status = SaveStatus::Saving;
        Ok(request)
    }

    /// An uncertain retry resends exactly the persisted ID, base, and document, not newer edits.
    pub fn retry_uncertain(&mut self) -> Result<SaveRequest, EditorError> {
        if self.status != SaveStatus::Uncertain {
            return Err(EditorError("No uncertain save is available to retry."));
        }
        let request = self
            .pending
            .as_ref()
            .ok_or(EditorError("Missing immutable save request."))?
            .clone();
        self.status = SaveStatus::Saving;
        Ok(request)
    }

    /// False means a stale completion; it cannot change this editor's newer request or draft.
    pub fn complete(
        &mut self,
        id: &str,
        result: Result<SaveReceipt, BackendError>,
    ) -> Result<bool, EditorError> {
        let Some(pending) = self.pending.as_ref().filter(|pending| pending.id == id) else {
            return Ok(false);
        };
        let expected = pending
            .expected_revision
            .checked_add(1)
            .ok_or(EditorError("Invalid pending workspace revision."))?;
        match result {
            Ok(SaveReceipt::Saved { revision }) if revision == expected => {
                self.base_revision = revision;
                self.status = if self.generation == pending.generation {
                    SaveStatus::Clean
                } else {
                    SaveStatus::Dirty
                };
                self.pending = None;
            }
            Ok(SaveReceipt::Superseded {
                revision,
                saved_revision,
            }) if saved_revision == expected
                && revision > saved_revision
                && revision <= MAX_REVISION =>
            {
                self.pending = None;
                self.status = SaveStatus::Conflict {
                    remote_revision: revision,
                };
            }
            Ok(SaveReceipt::Conflict { revision })
                if revision != pending.expected_revision && revision <= MAX_REVISION =>
            {
                self.pending = None;
                self.status = SaveStatus::Conflict {
                    remote_revision: revision,
                };
            }
            Err(BackendError::Unavailable | BackendError::Rejected) => {
                self.pending = None;
                self.status = SaveStatus::Rejected;
            }
            Err(_) => self.status = SaveStatus::Uncertain,
            Ok(_) => {
                self.status = SaveStatus::Uncertain;
                return Err(EditorError(
                    "The save acknowledgement does not match its immutable request.",
                ));
            }
        }
        Ok(true)
    }

    /// The UI presents the fresh remote layout before this explicit keep-local decision.
    /// A retired, still-running request remains protected by the server CAS and cannot
    /// overwrite any subsequently accepted revision. Its completion is ignored by ID.
    pub fn keep_local_after_reload(
        &mut self,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<(), EditorError> {
        if snapshot.revision > MAX_REVISION {
            return Err(EditorError("Invalid remote revision."));
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(EditorError("Workspace edit generation is exhausted."))?;
        self.pending = None;
        self.base_revision = snapshot.revision;
        self.generation = generation;
        self.status = SaveStatus::Dirty;
        Ok(())
    }

    pub fn use_remote_after_reload(
        &mut self,
        snapshot: WorkspaceSnapshot,
    ) -> Result<(), EditorError> {
        if snapshot.revision > MAX_REVISION {
            return Err(EditorError("Invalid remote revision."));
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(EditorError("Workspace edit generation is exhausted."))?;
        self.pending = None;
        self.document = snapshot.document;
        self.base_revision = snapshot.revision;
        self.generation = generation;
        self.status = SaveStatus::Clean;
        Ok(())
    }

    pub fn journal(&self) -> Result<Vec<u8>, EditorError> {
        let pending = self.pending.as_ref().map(|request| PendingJournal {
            id: request.id.clone(),
            expected_revision: request.expected_revision,
            generation: request.generation,
            document: std::str::from_utf8(request.document.original())
                .expect("validated JSON")
                .into(),
        });
        let status = match self.status {
            SaveStatus::Clean => JournalStatus::Clean,
            SaveStatus::Dirty | SaveStatus::Saving | SaveStatus::Uncertain => JournalStatus::Dirty,
            SaveStatus::Rejected => JournalStatus::Rejected,
            SaveStatus::Conflict { remote_revision } => JournalStatus::Conflict { remote_revision },
        };
        let value = Journal {
            version: 1,
            deployment: self.deployment.clone(),
            document: std::str::from_utf8(self.document.original())
                .expect("validated JSON")
                .into(),
            base_revision: self.base_revision,
            generation: self.generation,
            pending,
            status,
        };
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| EditorError("Could not serialize workspace recovery data."))?;
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err(EditorError(
                "Workspace recovery data exceeds its limit. Keep or export the draft.",
            ));
        }
        Ok(bytes)
    }

    pub fn restore_journal(deployment: &str, original: Arc<[u8]>) -> Result<Self, JournalFailure> {
        Self::read_journal(deployment, &original)
            .map_err(|error| JournalFailure { original, error })
    }

    fn read_journal(deployment: &str, bytes: &[u8]) -> Result<Self, EditorError> {
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err(EditorError("Workspace recovery data exceeds its limit."));
        }
        let journal: Journal = serde_json::from_slice(bytes).map_err(|_| {
            EditorError("Workspace recovery data is malformed or from a newer format.")
        })?;
        let canonical = TerminalBackend::new(deployment)
            .map_err(|_| EditorError("Invalid workspace deployment."))?;
        if journal.version != 1 || journal.deployment != canonical.deployment() {
            return Err(EditorError(
                "Recovered workspace belongs to another backend or format. Preserve it before choosing another layout.",
            ));
        }
        if journal.base_revision > MAX_REVISION {
            return Err(EditorError("Invalid recovered workspace revision."));
        }
        let document = WorkspaceDocument::restore(journal.document.into_bytes().into())
            .map_err(|_| EditorError("Recovered workspace layout is invalid."))?;
        let pending = journal
            .pending
            .map(|pending| {
                if pending.expected_revision != journal.base_revision
                    || pending.expected_revision >= MAX_REVISION
                    || pending.generation > journal.generation
                    || uuid::Uuid::parse_str(&pending.id).is_err()
                    || !matches!(journal.status, JournalStatus::Dirty)
                {
                    return Err(EditorError("Recovered workspace request is invalid."));
                }
                let document = WorkspaceDocument::restore(pending.document.into_bytes().into())
                    .map_err(|_| EditorError("The sent workspace layout is invalid."))?;
                Ok(SaveRequest {
                    id: pending.id,
                    expected_revision: pending.expected_revision,
                    generation: pending.generation,
                    document,
                })
            })
            .transpose()?;
        let status = if pending.is_some() {
            SaveStatus::Uncertain
        } else {
            match journal.status {
                JournalStatus::Clean => SaveStatus::Clean,
                JournalStatus::Dirty => SaveStatus::Dirty,
                JournalStatus::Rejected => SaveStatus::Rejected,
                JournalStatus::Conflict { remote_revision } if remote_revision <= MAX_REVISION => {
                    SaveStatus::Conflict { remote_revision }
                }
                _ => return Err(EditorError("Invalid recovered conflict revision.")),
            }
        };
        Ok(Self {
            deployment: journal.deployment,
            document,
            base_revision: journal.base_revision,
            generation: journal.generation,
            pending,
            status,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    deployment: String,
    document: String,
    base_revision: u64,
    generation: u64,
    pending: Option<PendingJournal>,
    status: JournalStatus,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingJournal {
    id: String,
    expected_revision: u64,
    generation: u64,
    document: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
enum JournalStatus {
    Clean,
    Dirty,
    Rejected,
    Conflict { remote_revision: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn editor() -> WorkspaceEditor {
        WorkspaceEditor::new(
            "http://127.0.0.1:3210",
            WorkspaceSnapshot::parse(Value::Null).unwrap(),
        )
        .unwrap()
    }
    fn add(editor: &mut WorkspaceEditor, id: &str) {
        let next = editor
            .document()
            .add_workspace(id, &format!("pane-{id}"), id, None, "", 0.0)
            .unwrap();
        editor.edit(next).unwrap();
    }

    #[test]
    fn delayed_save_acknowledgement_keeps_newer_edits_and_serializes_the_next_request() {
        let mut editor = editor();
        add(&mut editor, "first");
        let first = editor.begin_save().unwrap();
        add(&mut editor, "second");
        assert!(editor.begin_save().is_err());
        editor
            .complete(first.id(), Ok(SaveReceipt::Saved { revision: 1 }))
            .unwrap();
        assert_eq!(editor.status(), SaveStatus::Dirty);
        let second = editor.begin_save().unwrap();
        assert_ne!(first.id(), second.id());
        assert_eq!(second.expected_revision(), 1);
        assert_eq!(second.arguments()["nodes"].as_array().unwrap().len(), 2);
        assert!(
            !editor
                .complete(first.id(), Ok(SaveReceipt::Saved { revision: 1 }))
                .unwrap()
        );
        assert_eq!(editor.status(), SaveStatus::Saving);
    }

    #[test]
    fn uncertain_save_survives_restart_with_exact_request_and_newer_draft_separate() {
        let mut editor = editor();
        add(&mut editor, "sent");
        let request = editor.begin_save().unwrap();
        add(&mut editor, "unsent");
        editor
            .complete(request.id(), Err(BackendError::UnknownWrite))
            .unwrap();
        let journal = editor.journal().unwrap();
        let mut restored =
            WorkspaceEditor::restore_journal("http://127.0.0.1:3210/", journal.into()).unwrap();
        assert_eq!(restored.status(), SaveStatus::Uncertain);
        let retry = restored.retry_uncertain().unwrap();
        assert_eq!(retry.id(), request.id());
        assert_eq!(retry.arguments(), request.arguments());
        assert_eq!(
            restored.document().document()["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        restored
            .complete(
                retry.id(),
                Ok(SaveReceipt::Superseded {
                    revision: 3,
                    saved_revision: 1,
                }),
            )
            .unwrap();
        assert_eq!(
            restored.status(),
            SaveStatus::Conflict { remote_revision: 3 }
        );
        assert!(restored.begin_save().is_err());
    }

    #[test]
    fn invalid_or_other_deployment_recovery_bytes_are_retained() {
        let mut editor = editor();
        add(&mut editor, "keep");
        let journal: Arc<[u8]> = editor.journal().unwrap().into();
        let failure =
            WorkspaceEditor::restore_journal("http://127.0.0.1:9999", journal.clone()).unwrap_err();
        assert_eq!(failure.original, journal);
        for bad in [
            b"null".to_vec(),
            b"{}".to_vec(),
            b"not json".to_vec(),
            vec![0; MAX_JOURNAL_BYTES + 1],
        ] {
            let original: Arc<[u8]> = bad.into();
            assert_eq!(
                WorkspaceEditor::restore_journal("http://127.0.0.1:3210", original.clone())
                    .unwrap_err()
                    .original,
                original
            );
        }
    }

    #[test]
    fn explicit_reconciliation_retires_uncertain_callbacks_and_preserves_the_chosen_draft() {
        let mut editor = editor();
        add(&mut editor, "local");
        let old = editor.begin_save().unwrap();
        editor
            .complete(old.id(), Err(BackendError::UnknownWrite))
            .unwrap();
        let remote = WorkspaceSnapshot::parse(json!({"nodes":[],"revision":4})).unwrap();
        editor.keep_local_after_reload(&remote).unwrap();
        let new = editor.begin_save().unwrap();
        assert_eq!(new.expected_revision(), 4);
        assert_eq!(new.arguments()["nodes"][0]["id"], "local");
        assert!(
            !editor
                .complete(old.id(), Ok(SaveReceipt::Saved { revision: 1 }))
                .unwrap()
        );
        editor
            .complete(new.id(), Ok(SaveReceipt::Conflict { revision: 5 }))
            .unwrap();
        assert_eq!(editor.status(), SaveStatus::Conflict { remote_revision: 5 });
        editor
            .use_remote_after_reload(
                WorkspaceSnapshot::parse(json!({"nodes":[],"revision":5})).unwrap(),
            )
            .unwrap();
        assert_eq!(editor.status(), SaveStatus::Clean);
        assert!(editor.document().pane_ids().is_empty());
    }

    #[test]
    fn a_bad_acknowledgement_cannot_clear_the_only_recovery_request() {
        let mut editor = editor();
        add(&mut editor, "draft");
        let request = editor.begin_save().unwrap();
        assert!(
            editor
                .complete(request.id(), Ok(SaveReceipt::Saved { revision: 7 }))
                .is_err()
        );
        assert_eq!(editor.status(), SaveStatus::Uncertain);
        assert_eq!(
            editor.retry_uncertain().unwrap().arguments(),
            request.arguments()
        );
    }
}
