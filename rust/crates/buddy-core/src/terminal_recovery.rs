//! Ordered, deployment-bound recovery writes using the existing private atomic
//! config writer. All methods perform blocking I/O and belong on the I/O worker.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::config::{FileStamp, save_config_body_at};
use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};
use crate::terminal_backend::TerminalBackend;

const MAX_BYTES: usize = 8 * 1024 * 1024;
type Fingerprint = [u8; 32];

#[derive(Debug, Clone, Copy)]
pub enum JournalKind {
    Workspaces,
    Prompts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalWrite {
    Stored,
    Unchanged,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryError {
    Busy,
    Changed,
    Unsupported,
    InvalidPayload,
    Sequence,
    TooLarge,
    Io,
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "Another app is using the terminal recovery file. The draft remains in memory.",
            Self::Changed => "The terminal recovery file changed elsewhere. Reload and compare before saving.",
            Self::Unsupported => "The existing terminal recovery format is unsupported. Retain its original before replacing it.",
            Self::InvalidPayload => "The terminal recovery document has an invalid format or deployment.",
            Self::Sequence => "A terminal recovery sequence cannot be reused for different content.",
            Self::TooLarge => "The terminal recovery file exceeds eight MiB. The existing file is retained.",
            Self::Io => "The terminal recovery file could not be read or saved. Existing data is retained.",
        })
    }
}
impl std::error::Error for RecoveryError {}

pub struct RecoverySnapshot {
    pub original: Option<Arc<[u8]>>,
    pub needs_recovery: bool,
}
impl std::fmt::Debug for RecoverySnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoverySnapshot")
            .field("needs_recovery", &self.needs_recovery)
            .finish_non_exhaustive()
    }
}

struct State {
    expected: Option<Fingerprint>,
    last: Option<(u64, Fingerprint)>,
    needs_recovery: bool,
}

struct DiskSnapshot {
    original: Option<Arc<[u8]>>,
    stamp: Option<FileStamp>,
}

pub struct RecoveryStore {
    path: PathBuf,
    deployment: String,
    state: Mutex<State>,
}
impl std::fmt::Debug for RecoveryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryStore { .. }")
    }
}

fn fingerprint(bytes: &[u8]) -> Fingerprint {
    Sha256::digest(bytes).into()
}

fn valid(bytes: &[u8], deployment: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| {
        value["version"].as_u64() == Some(1) && value["deployment"].as_str() == Some(deployment)
    })
}

fn read(path: &Path) -> Result<DiskSnapshot, RecoveryError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if FileStamp::of(path).is_none() {
                Ok(DiskSnapshot {
                    original: None,
                    stamp: None,
                })
            } else {
                Err(RecoveryError::Changed)
            };
        }
        Err(_) => return Err(RecoveryError::Io),
    };
    let before = FileStamp::of(path).ok_or(RecoveryError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RecoveryError::Io)?;
    if bytes.len() > MAX_BYTES {
        return Err(RecoveryError::TooLarge);
    }
    if FileStamp::of(path) != Some(before.clone()) {
        return Err(RecoveryError::Changed);
    }
    Ok(DiskSnapshot {
        original: Some(bytes.into()),
        stamp: Some(before),
    })
}

impl RecoveryStore {
    /// The domain decoder must accept an existing record before it may be
    /// replaced. Unsupported original bytes stay protected until retained.
    pub fn open(
        config: &Path,
        deployment: &str,
        kind: JournalKind,
        validate_existing: impl FnOnce(&[u8]) -> bool,
    ) -> Result<(Self, RecoverySnapshot), RecoveryError> {
        if !config.is_absolute() {
            return Err(RecoveryError::InvalidPayload);
        }
        let deployment = TerminalBackend::new(deployment)
            .map_err(|_| RecoveryError::InvalidPayload)?
            .deployment()
            .to_string();
        let digest = fingerprint(deployment.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let name = match kind {
            JournalKind::Workspaces => "workspaces",
            JournalKind::Prompts => "prompts",
        };
        let parent = config.parent().ok_or(RecoveryError::InvalidPayload)?;
        std::fs::create_dir_all(parent).map_err(|_| RecoveryError::Io)?;
        let path = parent.join(format!("native-terminal-{name}-{digest}.json"));
        let lock = ConfigLock::acquire(&path, DEFAULT_TIMEOUT)
            .map_err(|_| RecoveryError::Io)?
            .ok_or(RecoveryError::Busy)?;
        let DiskSnapshot { original, .. } = read(&path)?;
        if !lock.is_held() {
            return Err(RecoveryError::Changed);
        }
        let needs_recovery = original
            .as_ref()
            .is_some_and(|body| !valid(body, &deployment) || !validate_existing(body));
        let expected = original.as_ref().map(|bytes| fingerprint(bytes));
        Ok((
            Self {
                path,
                deployment,
                state: Mutex::new(State {
                    expected,
                    last: None,
                    needs_recovery,
                }),
            },
            RecoverySnapshot {
                original,
                needs_recovery,
            },
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, sequence: u64, bytes: &[u8]) -> Result<JournalWrite, RecoveryError> {
        if bytes.len() > MAX_BYTES {
            return Err(RecoveryError::TooLarge);
        }
        if !valid(bytes, &self.deployment) {
            return Err(RecoveryError::InvalidPayload);
        }
        let body = std::str::from_utf8(bytes).map_err(|_| RecoveryError::InvalidPayload)?;
        let digest = fingerprint(bytes);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let lock = ConfigLock::acquire(&self.path, DEFAULT_TIMEOUT)
            .map_err(|_| RecoveryError::Io)?
            .ok_or(RecoveryError::Busy)?;
        let DiskSnapshot {
            original: current,
            stamp,
        } = read(&self.path)?;
        if current.as_ref().map(|bytes| fingerprint(bytes)) != state.expected {
            return Err(RecoveryError::Changed);
        }
        if state.needs_recovery {
            return Err(RecoveryError::Unsupported);
        }
        if let Some((last, previous)) = state.last {
            if sequence < last {
                return Ok(JournalWrite::Superseded);
            }
            if sequence == last {
                return if previous == digest {
                    Ok(JournalWrite::Unchanged)
                } else {
                    Err(RecoveryError::Sequence)
                };
            }
        }
        if !lock.is_held() {
            return Err(RecoveryError::Changed);
        }
        if !save_config_body_at(&self.path, body, Some(&stamp)).map_err(|_| RecoveryError::Io)? {
            return Err(RecoveryError::Changed);
        }
        if !lock.is_held() {
            return Err(RecoveryError::Changed);
        }
        state.expected = Some(digest);
        state.last = Some((sequence, digest));
        Ok(JournalWrite::Stored)
    }

    /// Call only after the user chooses to retain a recovery record before replacing it.
    /// The exact original is retained under a fresh name before writes are enabled.
    pub fn retain_original(&self) -> Result<Option<PathBuf>, RecoveryError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let lock = ConfigLock::acquire(&self.path, DEFAULT_TIMEOUT)
            .map_err(|_| RecoveryError::Io)?
            .ok_or(RecoveryError::Busy)?;
        let DiskSnapshot {
            original: current, ..
        } = read(&self.path)?;
        if current.as_ref().map(|bytes| fingerprint(bytes)) != state.expected {
            return Err(RecoveryError::Changed);
        }
        let Some(bytes) = current else {
            state.needs_recovery = false;
            return Ok(None);
        };
        let archive = self
            .path
            .with_extension(format!("retained-{}.json", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&archive).map_err(|_| RecoveryError::Io)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| RecoveryError::Io)?;
        if !lock.is_held() {
            return Err(RecoveryError::Changed);
        }
        state.needs_recovery = false;
        Ok(Some(archive))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "http://127.0.0.1:3210";

    fn open(
        config: &Path,
        deployment: &str,
        kind: JournalKind,
    ) -> Result<(RecoveryStore, RecoverySnapshot), RecoveryError> {
        RecoveryStore::open(config, deployment, kind, |_| true)
    }

    fn body(text: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":1,"deployment":URL,"draft":{"text":text}}))
            .unwrap()
    }

    #[test]
    fn domain_rejection_protects_a_header_compatible_original_and_retention_does_not_revive_old_writes()
     {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let (first, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        first.write(5, &body("original domain data")).unwrap();
        let archive = first.retain_original().unwrap().unwrap();
        assert_eq!(
            first.write(4, &body("stale queued write")).unwrap(),
            JournalWrite::Superseded
        );
        assert_eq!(
            std::fs::read(&archive).unwrap(),
            body("original domain data")
        );
        let (unsupported, snapshot) =
            RecoveryStore::open(&config, URL, JournalKind::Workspaces, |_| false).unwrap();
        assert!(snapshot.needs_recovery);
        assert_eq!(
            unsupported.write(1, &body("new document")),
            Err(RecoveryError::Unsupported)
        );
        assert_eq!(
            std::fs::read(unsupported.path()).unwrap(),
            body("original domain data")
        );
    }

    #[test]
    fn concurrent_instances_and_out_of_order_callbacks_cannot_overwrite_a_newer_draft() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let (first, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        let (second, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        assert_eq!(
            first.write(1, &body("first")).unwrap(),
            JournalWrite::Stored
        );
        assert_eq!(
            second.write(1, &body("other instance")),
            Err(RecoveryError::Changed)
        );
        assert_eq!(std::fs::read(first.path()).unwrap(), body("first"));
        assert_eq!(
            first.write(3, &body("newer")).unwrap(),
            JournalWrite::Stored
        );
        assert_eq!(
            first.write(2, &body("late older callback")).unwrap(),
            JournalWrite::Superseded
        );
        assert_eq!(
            first.write(3, &body("reused sequence")),
            Err(RecoveryError::Sequence)
        );
        assert_eq!(
            first.write(3, &body("newer")).unwrap(),
            JournalWrite::Unchanged
        );
        assert_eq!(std::fs::read(first.path()).unwrap(), body("newer"));
    }

    #[test]
    fn malformed_original_must_be_retained_exactly_before_it_can_be_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let (first, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        let path = first.path().to_path_buf();
        let original = b"unsupported recovery bytes \xff original user draft";
        std::fs::write(&path, original).unwrap();
        drop(first);
        let (store, snapshot) = open(&config, URL, JournalKind::Workspaces).unwrap();
        assert!(snapshot.needs_recovery);
        assert_eq!(snapshot.original.as_deref(), Some(&original[..]));
        assert_eq!(
            store.write(1, &body("replacement")),
            Err(RecoveryError::Unsupported)
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let archive = store.retain_original().unwrap().unwrap();
        assert_eq!(std::fs::read(&archive).unwrap(), original);
        assert_eq!(
            store.write(1, &body("replacement")).unwrap(),
            JournalWrite::Stored
        );
        assert_eq!(std::fs::read(&archive).unwrap(), original);
        assert_eq!(std::fs::read(&path).unwrap(), body("replacement"));
    }

    #[test]
    fn deployment_and_journal_kind_have_distinct_paths_and_accept_unicode_parent_directories() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory
            .path()
            .join("folder with spaces 界é")
            .join("config.json");
        let (a, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        let (b, _) = open(&config, URL, JournalKind::Prompts).unwrap();
        let (c, _) = open(&config, "http://127.0.0.1:3220", JournalKind::Workspaces).unwrap();
        assert_ne!(a.path(), b.path());
        assert_ne!(a.path(), c.path());
        assert_eq!(
            a.write(1, &body("local draft")).unwrap(),
            JournalWrite::Stored
        );
        assert_eq!(
            c.write(1, &body("wrong deployment")),
            Err(RecoveryError::InvalidPayload)
        );
        assert!(!b.path().exists());
        assert!(!c.path().exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(a.path()).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

    #[test]
    fn external_edits_are_retained_instead_of_being_silently_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = open(
            &directory.path().join("config.json"),
            URL,
            JournalKind::Prompts,
        )
        .unwrap();
        store.write(1, &body("local")).unwrap();
        std::fs::write(store.path(), body("external newer draft")).unwrap();
        assert_eq!(
            store.write(2, &body("stale process")),
            Err(RecoveryError::Changed)
        );
        assert_eq!(store.retain_original(), Err(RecoveryError::Changed));
        assert_eq!(
            std::fs::read(store.path()).unwrap(),
            body("external newer draft")
        );
    }

    #[test]
    fn oversized_recovery_file_is_bounded_and_preserved_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let (store, _) = open(&config, URL, JournalKind::Workspaces).unwrap();
        let oversized = vec![b'x'; MAX_BYTES + 1];
        assert_eq!(store.write(1, &oversized), Err(RecoveryError::TooLarge));
        std::fs::write(store.path(), &oversized).unwrap();
        assert!(matches!(
            open(&config, URL, JournalKind::Workspaces),
            Err(RecoveryError::TooLarge)
        ));
        assert_eq!(
            std::fs::metadata(store.path()).unwrap().len(),
            oversized.len() as u64
        );
    }
}
