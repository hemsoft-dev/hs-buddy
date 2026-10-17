//! The application boundary for persisted, explicitly dispatched user changes.
//! Run these operations on a worker: filesystem locking must not block GPUI.
use std::path::{Path, PathBuf};

use super::storage::{PendingFile, StorageError};
use super::{
    Client, Error, Id, TransportError, WriteAttempt, WriteOutcome, WritePhase, WriteReply,
    WriteRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeError {
    Storage(StorageError),
    Transport(TransportError),
}
impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ChangeError {}
impl From<StorageError> for ChangeError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
impl From<TransportError> for ChangeError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

pub struct DispatchReport {
    pub phase: WritePhase,
    pub task: Option<Id>,
    pub transport_problem: Option<TransportError>,
    /// A definite remote acknowledgement survives failure to retire the file.
    pub cleanup_problem: Option<StorageError>,
    pub acknowledgement_durable: bool,
}

/// No Debug: owns a private immutable task command and its recovery bytes.
pub struct SavedChange {
    config: PathBuf,
    file: PendingFile,
    attempt: WriteAttempt,
    acknowledgement_durable: bool,
}
impl SavedChange {
    pub(super) fn recovery_bytes(&self) -> Option<&[u8]> {
        self.file.current()
    }

    /// No write is sent. Verify the account and publish recovery before returning.
    pub async fn capture(
        client: &Client,
        config: &Path,
        request: WriteRequest,
    ) -> Result<Self, ChangeError> {
        Self::capture_with_account(client, config, request, None).await
    }

    pub async fn capture_for_account(
        client: &Client,
        config: &Path,
        request: WriteRequest,
        account: &Id,
    ) -> Result<Self, ChangeError> {
        Self::capture_with_account(client, config, request, Some(account)).await
    }

    async fn capture_with_account(
        client: &Client,
        config: &Path,
        request: WriteRequest,
        account: Option<&Id>,
    ) -> Result<Self, ChangeError> {
        let mut file = PendingFile::open(config)?;
        if file.current().is_some() {
            return Err(StorageError::Pending.into());
        }
        let attempt = WriteAttempt::new(request);
        let bytes = match account {
            Some(account) => {
                client
                    .checkpoint_for_account(&attempt, config, account)
                    .await?
            }
            None => client.checkpoint(&attempt, config).await?,
        };
        file.persist(&bytes)?;
        Ok(Self {
            config: config.to_path_buf(),
            file,
            attempt,
            acknowledgement_durable: false,
        })
    }

    /// Recovery only reads identity; it never dispatches a saved change.
    pub async fn recover(client: &Client, config: &Path) -> Result<Option<Self>, ChangeError> {
        Self::recover_with_account(client, config, None).await
    }

    pub async fn recover_for_account(
        client: &Client,
        config: &Path,
        account: &Id,
    ) -> Result<Option<Self>, ChangeError> {
        Self::recover_with_account(client, config, Some(account)).await
    }

    async fn recover_with_account(
        client: &Client,
        config: &Path,
        account: Option<&Id>,
    ) -> Result<Option<Self>, ChangeError> {
        let file = PendingFile::open(config)?;
        let Some(bytes) = file.current() else {
            return Ok(None);
        };
        let attempt = match account {
            Some(account) => client.recover_for_account(bytes, config, account).await?,
            None => client.recover(bytes, config).await?,
        };
        let acknowledgement_durable = matches!(
            attempt.phase(),
            WritePhase::Confirmed | WritePhase::Rejected
        );
        Ok(Some(Self {
            config: config.to_path_buf(),
            file,
            attempt,
            acknowledgement_durable,
        }))
    }

    pub fn phase(&self) -> WritePhase {
        self.attempt.phase()
    }

    pub fn acknowledgement_durable(&self) -> bool {
        self.acknowledgement_durable
    }

    /// Inspect the original private command for recovery presentation. This does
    /// not begin a send, change its phase, or perform filesystem/network I/O.
    pub fn request(&self) -> &WriteRequest {
        self.attempt.request()
    }

    /// Each call is an explicit user send/retry. Errors returned before dispatch
    /// leave the immutable recovery in place. After dispatch, all uncertainty is
    /// represented in the report and the pending record remains available.
    pub async fn dispatch(&mut self, client: &Client) -> Result<DispatchReport, ChangeError> {
        if !matches!(self.phase(), WritePhase::Ready | WritePhase::Unconfirmed) {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        let bytes = client.checkpoint(&self.attempt, &self.config).await?;
        // A token's authenticated account may have changed since capture.
        if self.file.current() != Some(bytes.as_slice()) {
            return Err(TransportError::Domain(Error::WrongConnection).into());
        }
        self.file.persist(&bytes)?;
        let request = self
            .attempt
            .begin(client.write_scope())
            .map_err(TransportError::from)?;
        let reply = match client.send(&request).await {
            Ok(reply) => reply,
            Err(problem) => WriteReply {
                outcome: WriteOutcome::Unconfirmed,
                problem: Some(problem),
            },
        };
        self.attempt.observe(request.command_id(), reply.outcome);
        let cleanup_problem =
            if matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected) {
                self.finish_acknowledged().err().map(|error| match error {
                    ChangeError::Storage(error) => error,
                    ChangeError::Transport(_) => StorageError::InvalidCheckpoint,
                })
            } else {
                None
            };
        Ok(DispatchReport {
            phase: self.phase(),
            task: self.attempt.task_id().cloned(),
            transport_problem: reply.problem,
            cleanup_problem,
            acknowledgement_durable: self.acknowledgement_durable,
        })
    }

    /// Retry local cleanup only. Never contacts Todoist or replays a command.
    pub fn retry_cleanup(&mut self) -> Result<(), ChangeError> {
        if !matches!(self.phase(), WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        self.finish_acknowledged()
    }

    fn finish_acknowledged(&mut self) -> Result<(), ChangeError> {
        let current = self.file.current().ok_or(StorageError::InvalidCheckpoint)?;
        let acknowledgement = match self.attempt.acknowledgement(current) {
            Ok(marker) => marker,
            Err(_) => {
                // An interrupted append owned by this instance is invalid on
                // restart, but its known in-memory outcome still permits local
                // retirement. The exact-byte guard never deletes foreign data.
                self.file.retire()?;
                self.acknowledgement_durable = true;
                return Ok(());
            }
        };
        if let Some(acknowledgement) = acknowledgement
            && let Err(persist_error) = self.file.append_acknowledgement(&acknowledgement)
        {
            // If the outcome cannot be saved but the original file can be
            // retired, retirement also prevents a later replay. Both failures
            // leave the known outcome visible for local-only cleanup.
            self.acknowledgement_durable = false;
            return match self.file.retire() {
                Ok(()) => {
                    self.acknowledgement_durable = true;
                    Ok(())
                }
                Err(_) => Err(persist_error.into()),
            };
        }
        self.acknowledgement_durable = true;
        self.file.retire().map_err(Into::into)
    }

    /// Only after explicit confirmation that the remote change may already
    /// exist. Remove this exact owned record without sending any service write.
    /// A changed or unavailable file remains visible as a cleanup failure.
    pub fn discard(&mut self) -> Result<(), ChangeError> {
        self.file.retire().map_err(Into::into)
    }
}
