//! Todoist keeps its existing recovery slot and protocol-specific decoder.
pub use crate::pending_file::StorageError;
use crate::pending_file::{PendingFile as SharedPendingFile, RecoverySlot};
use std::path::Path;

pub struct PendingFile {
    inner: SharedPendingFile,
}
impl PendingFile {
    pub fn open(config: &Path) -> Result<Self, StorageError> {
        Ok(Self {
            inner: SharedPendingFile::open(
                config,
                RecoverySlot::Todoist,
                super::MAX_CHECKPOINT_BYTES,
            )?,
        })
    }
    pub fn current(&self) -> Option<&[u8]> {
        self.inner.current()
    }
    pub fn persist(&mut self, checkpoint: &[u8]) -> Result<(), StorageError> {
        self.inner.persist(checkpoint)
    }
    pub fn retire(&mut self) -> Result<(), StorageError> {
        self.inner.retire()
    }
    pub(super) fn while_empty<T>(&self, action: impl FnOnce() -> T) -> Result<T, StorageError> {
        self.inner.while_empty(action)
    }
    pub(super) fn while_matching<T>(
        &self,
        expected: Option<&[u8]>,
        action: impl FnOnce() -> T,
    ) -> Result<T, StorageError> {
        self.inner.while_matching(expected, action)
    }
    pub(super) fn append_acknowledgement(
        &mut self,
        acknowledgement: &[u8],
    ) -> Result<(), StorageError> {
        self.inner.append_acknowledgement(acknowledgement)
    }
    #[cfg(test)]
    fn path(&self) -> &Path {
        self.inner.path()
    }
}
#[cfg(test)]
use super::MAX_CHECKPOINT_BYTES;
#[cfg(test)]
use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};
#[cfg(test)]
use std::{fs, path::PathBuf};
#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
