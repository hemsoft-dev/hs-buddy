//! One immutable pending change per configuration profile. Store the client's
//! validated checkpoint before dispatch; read/authenticate it again on recovery.
//! Filesystem failures block dispatch. No failure permits an implicit resend.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};

use super::MAX_CHECKPOINT_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    Unavailable,
    Busy,
    Changed,
    Pending,
    TooLarge,
    InvalidPath,
    InvalidCheckpoint,
}
impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "The Todoist recovery file could not be confirmed. Do not send the change; reopen recovery first.",
            Self::Busy => "Another window is using Todoist recovery. Wait and reopen it before sending a change.",
            Self::Changed => "Todoist recovery changed in another window. Reopen it before continuing.",
            Self::Pending => "Resolve the saved Todoist change before preparing another one.",
            Self::TooLarge => "The Todoist recovery file exceeds the supported size. Keep it for inspection.",
            Self::InvalidPath => "The Todoist recovery location is unsupported. The existing file was preserved.",
            Self::InvalidCheckpoint => "The Todoist change has no recovery data. Keep the draft and do not send it.",
        })
    }
}
impl std::error::Error for StorageError {}

/// No Debug: the captured bytes can contain private task text. This type owns
/// storage only; Client::recover verifies account identity and command shape.
pub struct PendingFile {
    path: PathBuf,
    original: Option<Vec<u8>>,
}
impl PendingFile {
    pub fn open(config: &Path) -> Result<Self, StorageError> {
        if !config.is_absolute() || config.file_name().is_none() {
            return Err(StorageError::InvalidPath);
        }
        let identity = crate::pollen_credentials::account_for_path(config)
            .map_err(|_| StorageError::InvalidPath)?;
        let parent = config.parent().ok_or(StorageError::InvalidPath)?;
        fs::create_dir_all(parent).map_err(|_| StorageError::Unavailable)?;
        let path = parent.join(format!("native-todoist-change-{identity}.json"));
        let lock = lock(&path)?;
        let original = read(&path)?;
        held(&lock)?;
        Ok(Self { path, original })
    }

    pub fn current(&self) -> Option<&[u8]> {
        self.original.as_deref()
    }

    /// Serialize protected-token removal with publication of recovery records.
    /// The caller's action and absence check share the same exclusive lease.
    pub(super) fn while_empty<T>(&self, action: impl FnOnce() -> T) -> Result<T, StorageError> {
        let lock = lock(&self.path)?;
        if read(&self.path)?.is_some() {
            return Err(StorageError::Pending);
        }
        held(&lock)?;
        let result = action();
        held(&lock)?;
        Ok(result)
    }

    pub(super) fn while_matching<T>(
        &self,
        expected: Option<&[u8]>,
        action: impl FnOnce() -> T,
    ) -> Result<T, StorageError> {
        let lock = lock(&self.path)?;
        if read(&self.path)?.as_deref() != expected {
            return Err(StorageError::Changed);
        }
        held(&lock)?;
        let result = action();
        held(&lock)?;
        Ok(result)
    }

    /// Persist a terminal service outcome without needing directory deletion
    /// permission. An interrupted append is invalid recovery, never permission
    /// to resend an acknowledged change. The command itself remains unchanged.
    pub(super) fn append_acknowledgement(
        &mut self,
        acknowledgement: &[u8],
    ) -> Result<(), StorageError> {
        let original = self
            .original
            .as_ref()
            .ok_or(StorageError::InvalidCheckpoint)?;
        if acknowledgement.is_empty() || acknowledgement.len() > 4096 {
            return Err(StorageError::InvalidCheckpoint);
        }
        let mut expected = original.clone();
        // The first appended byte is '{'. Even a one-byte interrupted append
        // must be invalid recovery, never ignorable trailing whitespace.
        expected.extend_from_slice(acknowledgement);
        if expected.len() > MAX_CHECKPOINT_BYTES {
            return Err(StorageError::TooLarge);
        }
        let lock = lock(&self.path)?;
        self.unchanged(&lock)?;
        let mut options = OpenOptions::new();
        options.read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let mut file = options
            .open(&self.path)
            .map_err(|_| StorageError::Unavailable)?;
        let metadata = file.metadata().map_err(|_| StorageError::Unavailable)?;
        if !metadata.is_file() {
            return Err(StorageError::InvalidPath);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(StorageError::InvalidPath);
            }
        }
        let mut observed = Vec::new();
        (&mut file)
            .take(MAX_CHECKPOINT_BYTES as u64 + 1)
            .read_to_end(&mut observed)
            .map_err(|_| StorageError::Unavailable)?;
        if &observed != original {
            return Err(StorageError::Changed);
        }
        held(&lock)?;
        let result = file
            .write_all(&expected[original.len()..])
            .and_then(|_| file.sync_all());
        drop(file);
        // Retain an owned partial append for local-only cleanup after a failed
        // write. A different writer's content never becomes ours to retire.
        let actual = read(&self.path)?;
        if let Some(actual) = &actual
            && actual.starts_with(original)
            && expected.starts_with(actual)
        {
            self.original = Some(actual.clone());
        }
        held(&lock)?;
        result.map_err(|_| StorageError::Unavailable)?;
        if actual.as_deref() != Some(expected.as_slice()) {
            return Err(StorageError::Changed);
        }
        Ok(())
    }

    /// Publishes fully written, flushed bytes without replacing another file.
    /// A filesystem without hard-link support fails closed. Power-loss behavior
    /// still depends on the filesystem; this is not a provider acknowledgement.
    pub fn persist(&mut self, checkpoint: &[u8]) -> Result<(), StorageError> {
        if checkpoint.is_empty() {
            return Err(StorageError::InvalidCheckpoint);
        }
        if checkpoint.len() > MAX_CHECKPOINT_BYTES {
            return Err(StorageError::TooLarge);
        }
        let lock = lock(&self.path)?;
        self.unchanged(&lock)?;
        if let Some(original) = &self.original {
            return if original == checkpoint {
                Ok(())
            } else {
                Err(StorageError::Pending)
            };
        }
        let staging = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&staging)
            .map_err(|_| StorageError::Unavailable)?;
        // Only this successfully created, unpredictable staging name is ours.
        let staging = Staging(staging);
        file.write_all(checkpoint)
            .and_then(|_| file.sync_all())
            .map_err(|_| StorageError::Unavailable)?;
        drop(file);
        held(&lock)?;
        fs::hard_link(&staging.0, &self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                StorageError::Changed
            } else {
                StorageError::Unavailable
            }
        })?;
        sync_parent(&self.path)?;
        if read(&self.path)?.as_deref() != Some(checkpoint) {
            return Err(StorageError::Changed);
        }
        held(&lock)?;
        self.original = Some(checkpoint.to_vec());
        Ok(())
    }

    /// Call after a confirmed/rejected remote result, or explicit user discard.
    /// A changed file is never removed. An IO failure remains unconfirmed.
    pub fn retire(&mut self) -> Result<(), StorageError> {
        let lock = lock(&self.path)?;
        self.unchanged(&lock)?;
        if self.original.is_some() {
            fs::remove_file(&self.path).map_err(|_| StorageError::Unavailable)?;
            sync_parent(&self.path)?;
            held(&lock)?;
            self.original = None;
        }
        Ok(())
    }

    fn unchanged(&self, lock: &ConfigLock) -> Result<(), StorageError> {
        if read(&self.path)? != self.original {
            return Err(StorageError::Changed);
        }
        held(lock)
    }
}

fn lock(path: &Path) -> Result<ConfigLock, StorageError> {
    ConfigLock::acquire(path, DEFAULT_TIMEOUT)
        .map_err(|_| StorageError::Unavailable)?
        .filter(ConfigLock::is_held)
        .ok_or(StorageError::Busy)
}
fn held(lock: &ConfigLock) -> Result<(), StorageError> {
    if lock.is_held() {
        Ok(())
    } else {
        Err(StorageError::Changed)
    }
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StorageError::Unavailable),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StorageError::InvalidPath);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(StorageError::InvalidPath);
        }
    }
    if metadata.len() > MAX_CHECKPOINT_BYTES as u64 {
        return Err(StorageError::TooLarge);
    }
    let file = File::open(path).map_err(|_| StorageError::Unavailable)?;
    let mut bytes = Vec::new();
    file.take(MAX_CHECKPOINT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| StorageError::Unavailable)?;
    if bytes.len() > MAX_CHECKPOINT_BYTES {
        return Err(StorageError::TooLarge);
    }
    Ok(Some(bytes))
}
fn sync_parent(path: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    File::open(path.parent().ok_or(StorageError::InvalidPath)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| StorageError::Unavailable)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
