//! Persist before sending. Restored Tempo changes have no dispatch operation.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::pending_file::{PendingFile, RecoverySlot};

use super::{
    Client, Draft, DraftKind, Error, JiraClient, StorageError, TransportError, WorklogId,
    WriteAttempt, WriteOutcome, WritePhase, recovery,
};

const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const _: () = assert!(WRITE_TIMEOUT.as_secs() < crate::config_lock::STALE_AFTER.as_secs());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeError {
    Storage(StorageError),
    Recovery(recovery::RecoveryError),
    Transport(TransportError),
    Domain(Error),
}
impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Recovery(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
            Self::Domain(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ChangeError {}
impl From<StorageError> for ChangeError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}
impl From<recovery::RecoveryError> for ChangeError {
    fn from(value: recovery::RecoveryError) -> Self {
        Self::Recovery(value)
    }
}
impl From<TransportError> for ChangeError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}
impl From<Error> for ChangeError {
    fn from(value: Error) -> Self {
        Self::Domain(value)
    }
}

pub(super) struct WriteReply {
    pub outcome: WriteOutcome,
    pub problem: Option<TransportError>,
}
impl WriteReply {
    fn unconfirmed(problem: TransportError) -> Self {
        Self {
            outcome: WriteOutcome::Unconfirmed,
            problem: Some(problem),
        }
    }
}
pub struct DispatchReport {
    pub phase: WritePhase,
    pub worklog: Option<WorklogId>,
    pub transport_problem: Option<TransportError>,
    pub cleanup_problem: Option<StorageError>,
    pub acknowledgement_durable: bool,
}

/// Captured clients and profile cannot change while a prepared operation exists.
pub struct SavedChange {
    client: Arc<Client>,
    jira: Arc<JiraClient>,
    config: PathBuf,
    file: PendingFile,
    checkpoint: Vec<u8>,
    attempt: WriteAttempt,
    acknowledgement_durable: bool,
}
impl SavedChange {
    /// Identity verification and flushed recovery publication perform no HTTP write.
    pub async fn capture(
        client: Arc<Client>,
        jira: Arc<JiraClient>,
        config: &Path,
        draft: Draft,
    ) -> Result<Self, ChangeError> {
        let mut file = PendingFile::open(config, RecoverySlot::Tempo, recovery::MAX_STORED_BYTES)?;
        if file.current().is_some() {
            return Err(StorageError::Pending.into());
        }
        let attempt = client.capture(draft)?;
        let checkpoint = client.checkpoint(&attempt, config, &jira).await?;
        file.persist(&checkpoint)?;
        Ok(Self {
            client,
            jira,
            config: config.to_path_buf(),
            file,
            checkpoint,
            attempt,
            acknowledgement_durable: false,
        })
    }
    pub fn phase(&self) -> WritePhase {
        self.attempt.phase()
    }
    pub fn draft(&self) -> &Draft {
        // Both accessors validate the same immutable captured connection.
        if matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected) {
            self.attempt
                .terminal_request(self.client.write_scope())
                .expect("captured terminal request")
                .draft()
        } else {
            self.attempt
                .pending_request(self.client.write_scope())
                .expect("captured pending request")
                .draft()
        }
    }
    pub fn acknowledgement_durable(&self) -> bool {
        self.acknowledgement_durable
    }

    /// Only the original Ready operation can send. No uncertainty permits replay.
    pub async fn dispatch(&mut self) -> Result<DispatchReport, ChangeError> {
        if self.phase() != WritePhase::Ready {
            return Err(Error::WriteBlocked.into());
        }
        let checkpoint = self
            .client
            .checkpoint(&self.attempt, &self.config, &self.jira)
            .await?;
        if checkpoint != self.checkpoint || self.file.current() != Some(checkpoint.as_slice()) {
            return Err(Error::WrongConnection.into());
        }
        let request = self.attempt.pending_request(self.client.write_scope())?;
        if request.draft().kind() != DraftKind::Create {
            let id = request.draft().worklog_id().ok_or(Error::InvalidDraft)?;
            let author = request.draft().author();
            let expanded = request
                .draft()
                .baseline()
                .is_some_and(|row| row.attributes.is_some());
            let mut current = if expanded {
                self.client.worklog_for_edit(author, id).await?
            } else {
                self.client.worklog(author, id).await?
            };
            if !expanded {
                // Deletion captured no attributes to overwrite. A detail read
                // may expand them without changing any displayed field.
                current.attributes = None;
            }
            request.check_update_baseline(&current)?;
        }
        self.file.persist(&checkpoint)?;
        let lease = self.file.lease_pending()?;
        lease.check()?;
        let request = self.attempt.begin(self.client.write_scope())?;
        let reply =
            match tokio::time::timeout(WRITE_TIMEOUT, self.client.send_captured(&request)).await {
                Ok(Ok(reply)) => reply,
                Ok(Err(error)) => WriteReply::unconfirmed(error),
                Err(_) => WriteReply::unconfirmed(TransportError::Unavailable),
            };
        self.attempt.observe(request.operation_id(), reply.outcome);
        let mut cleanup_problem = lease.check().err();
        drop(lease);
        if matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected)
            && let Err(error) = self.finish_acknowledged()
        {
            cleanup_problem.get_or_insert(error);
        }
        Ok(DispatchReport {
            phase: self.phase(),
            worklog: self.attempt.confirmed_id(),
            transport_problem: reply.problem,
            cleanup_problem,
            acknowledgement_durable: self.acknowledgement_durable,
        })
    }

    fn finish_acknowledged(&mut self) -> Result<(), StorageError> {
        if self.acknowledgement_durable {
            return self.file.retire();
        }
        if self.file.current() != Some(self.checkpoint.as_slice()) {
            // This instance may own an interrupted append. Never append a second
            // marker to it; exact-byte retirement cannot remove a foreign file.
            self.file.retire()?;
            self.acknowledgement_durable = true;
            return Ok(());
        }
        let marker =
            recovery::acknowledgement(&self.attempt, self.client.write_scope(), &self.checkpoint)
                .map_err(|_| StorageError::InvalidCheckpoint)?;
        if let Err(error) = self.file.append_acknowledgement(&marker) {
            return match self.file.retire() {
                Ok(()) => {
                    self.acknowledgement_durable = true;
                    Ok(())
                }
                Err(_) => Err(error),
            };
        }
        self.acknowledgement_durable = true;
        self.file.retire()
    }
    /// Retry local retirement only, never another service request.
    pub fn retry_cleanup(&mut self) -> Result<(), ChangeError> {
        if !matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked.into());
        }
        self.finish_acknowledged().map_err(Into::into)
    }
    /// An explicit local discard cannot undo a service write.
    pub fn discard(&mut self) -> Result<(), ChangeError> {
        if !matches!(self.phase(), WritePhase::Ready | WritePhase::Unconfirmed) {
            return Err(Error::WriteBlocked.into());
        }
        self.file.retire().map_err(Into::into)
    }
}

/// No dispatch method or Ready conversion exists on a restored record.
pub struct SavedRecovery {
    file: PendingFile,
    recovered: recovery::RecoveredChange,
}
impl SavedRecovery {
    /// Exact authenticated bytes for guarded credential replacement.
    /// This does not grant any right to replay the restored request.
    pub fn stored_checkpoint(&self) -> Option<&[u8]> {
        self.file.current()
    }
    pub async fn open(config: &Path, jira: &JiraClient) -> Result<Option<Self>, ChangeError> {
        let file = PendingFile::open(config, RecoverySlot::Tempo, recovery::MAX_STORED_BYTES)?;
        let Some(bytes) = file.current() else {
            return Ok(None);
        };
        let recovered = recovery::inspect(bytes, config, jira).await?;
        Ok(Some(Self { file, recovered }))
    }
    pub fn phase(&self) -> WritePhase {
        self.recovered.phase()
    }
    pub fn draft(&self) -> &Draft {
        self.recovered.draft()
    }
    pub fn worklog(&self) -> Option<WorklogId> {
        self.recovered.confirmed_id()
    }
    pub fn cleanup(&mut self) -> Result<(), ChangeError> {
        if !matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked.into());
        }
        self.file.retire().map_err(Into::into)
    }
    pub fn discard(&mut self) -> Result<(), ChangeError> {
        if self.phase() != WritePhase::Unconfirmed {
            return Err(Error::WriteBlocked.into());
        }
        self.file.retire().map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "change_tests.rs"]
mod tests;
