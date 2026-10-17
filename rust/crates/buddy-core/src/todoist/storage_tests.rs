use super::*;
use crate::todoist::{Error, Id, WriteAttempt, WritePhase, WriteRequest, WriteScope};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("buddy-todoist-storage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn config(&self) -> PathBuf {
        self.0.join("config.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // A fresh UUID directory created by this test, never an owner profile.
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let resolved = self.0.canonicalize().unwrap();
        assert_eq!(resolved.parent(), Some(parent.as_path()));
        assert!(
            !fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn account() -> Id {
    Id::parse("synthetic-account").unwrap()
}
fn checkpoint(config: &Path, task: &str) -> Vec<u8> {
    WriteAttempt::new(WriteRequest::complete(WriteScope::new(), &Id::parse(task).unwrap()).unwrap())
        .checkpoint(config, &account())
        .unwrap()
}

#[test]
fn restart_recovers_exact_command_as_unconfirmed_without_dispatch() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let bytes = checkpoint(&config, "private-task");
    let mut file = PendingFile::open(&config).unwrap();
    assert!(file.current().is_none());
    file.persist(&bytes).unwrap();
    file.persist(&bytes).unwrap();
    drop(file);
    let restored = PendingFile::open(&config).unwrap();
    assert_eq!(restored.current(), Some(bytes.as_slice()));
    let scope = WriteScope::new();
    let mut attempt =
        WriteAttempt::restore(restored.current().unwrap(), &config, &account(), scope).unwrap();
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
    assert_eq!(attempt.checkpoint(&config, &account()).unwrap(), bytes);
    let sent = attempt.begin(scope).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sent.commands(), original["commands"].as_str().unwrap());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn stale_windows_cannot_replace_or_retire_another_pending_change() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let first = checkpoint(&config, "first");
    let second = checkpoint(&config, "second");
    let mut winner = PendingFile::open(&config).unwrap();
    let mut stale = PendingFile::open(&config).unwrap();
    winner.persist(&first).unwrap();
    assert_eq!(stale.persist(&second), Err(StorageError::Changed));
    assert_eq!(stale.retire(), Err(StorageError::Changed));
    assert_eq!(winner.persist(&second), Err(StorageError::Pending));
    assert_eq!(fs::read(&winner.path).unwrap(), first);
    winner.retire().unwrap();
    assert!(PendingFile::open(&config).unwrap().current().is_none());
}

#[test]
fn malformed_recovery_stays_available_and_cannot_be_overwritten() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut file = PendingFile::open(&config).unwrap();
    let invalid = b"partial interrupted old file";
    fs::write(&file.path, invalid).unwrap();
    assert_eq!(
        file.persist(&checkpoint(&config, "task")),
        Err(StorageError::Changed)
    );
    file = PendingFile::open(&config).unwrap();
    assert_eq!(file.current(), Some(invalid.as_slice()));
    assert!(matches!(
        WriteAttempt::restore(
            file.current().unwrap(),
            &config,
            &account(),
            WriteScope::new()
        ),
        Err(Error::InvalidRecovery)
    ));
    assert_eq!(
        file.persist(&checkpoint(&config, "task")),
        Err(StorageError::Pending)
    );
    assert_eq!(fs::read(&file.path).unwrap(), invalid);
    fs::write(&file.path, b"changed elsewhere").unwrap();
    assert_eq!(file.retire(), Err(StorageError::Changed));
    assert_eq!(fs::read(&file.path).unwrap(), b"changed elsewhere");
}

#[test]
fn profiles_have_distinct_recovery_slots_and_accounts_are_verified_separately() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut first = PendingFile::open(&config).unwrap();
    let other_config = fixture.0.join("other-config.json");
    let mut second = PendingFile::open(&other_config).unwrap();
    assert_ne!(first.path, second.path);
    let bytes = checkpoint(&config, "task");
    first.persist(&bytes).unwrap();
    second.persist(&checkpoint(&other_config, "other")).unwrap();
    assert!(matches!(
        WriteAttempt::restore(
            &bytes,
            &config,
            &Id::parse("wrong-account").unwrap(),
            WriteScope::new()
        ),
        Err(Error::WrongConnection)
    ));
    assert!(matches!(
        WriteAttempt::restore(&bytes, &other_config, &account(), WriteScope::new()),
        Err(Error::WrongConnection)
    ));
    first.retire().unwrap();
    assert!(
        PendingFile::open(&other_config)
            .unwrap()
            .current()
            .is_some()
    );
}

#[test]
fn busy_recovery_does_not_write_or_remove_any_change() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut file = PendingFile::open(&config).unwrap();
    let held = ConfigLock::acquire(&file.path, DEFAULT_TIMEOUT)
        .unwrap()
        .unwrap();
    assert_eq!(
        file.persist(&checkpoint(&config, "task")),
        Err(StorageError::Busy)
    );
    assert_eq!(file.retire(), Err(StorageError::Busy));
    assert!(!file.path.exists());
    drop(held);
    file.persist(&checkpoint(&config, "task")).unwrap();
}

#[test]
fn credential_removal_holds_recovery_lock_until_the_delete_finishes() {
    use std::sync::mpsc;
    let fixture = Fixture::new();
    let config = fixture.config();
    let held_config = config.clone();
    let mut publisher = PendingFile::open(&config).unwrap();
    let (entered, deleting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let file = PendingFile::open(&held_config).unwrap();
        file.while_empty(|| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            // A private callback represents the vault operation; no OS vault.
            42
        })
        .unwrap()
    });
    deleting
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let bytes = checkpoint(&config, "new-task");
    assert_eq!(publisher.persist(&bytes), Err(StorageError::Busy));
    assert!(!publisher.path.exists());
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), 42);
    publisher.persist(&bytes).unwrap();
    assert_eq!(fs::read(&publisher.path).unwrap(), bytes);
}

#[test]
fn credential_removal_rechecks_absence_after_another_window_publishes() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let stale = PendingFile::open(&config).unwrap();
    let bytes = checkpoint(&config, "other-window-task");
    let mut publisher = PendingFile::open(&config).unwrap();
    publisher.persist(&bytes).unwrap();
    let called = std::cell::Cell::new(false);
    assert_eq!(
        stale.while_empty(|| called.set(true)),
        Err(StorageError::Pending)
    );
    assert!(!called.get());
    assert_eq!(fs::read(&publisher.path).unwrap(), bytes);
}

#[test]
fn unsupported_paths_and_oversized_records_are_preserved() {
    assert!(matches!(
        PendingFile::open(Path::new("relative-config.json")),
        Err(StorageError::InvalidPath)
    ));
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut file = PendingFile::open(&config).unwrap();
    assert_eq!(file.persist(&[]), Err(StorageError::InvalidCheckpoint));
    assert_eq!(
        file.persist(&vec![0; MAX_CHECKPOINT_BYTES + 1]),
        Err(StorageError::TooLarge)
    );
    assert!(!file.path.exists());
    fs::write(&file.path, vec![1; MAX_CHECKPOINT_BYTES + 1]).unwrap();
    assert!(matches!(
        PendingFile::open(&config),
        Err(StorageError::TooLarge)
    ));
    assert_eq!(
        fs::metadata(&file.path).unwrap().len(),
        MAX_CHECKPOINT_BYTES as u64 + 1
    );
    fs::remove_file(&file.path).unwrap();
    fs::create_dir(&file.path).unwrap();
    assert_eq!(
        file.persist(&checkpoint(&config, "task")),
        Err(StorageError::InvalidPath)
    );
    assert!(file.path.is_dir());
}

#[cfg(unix)]
#[test]
fn new_records_are_private_and_symlinks_are_preserved_without_reading() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut file = PendingFile::open(&config).unwrap();
    file.persist(&checkpoint(&config, "task")).unwrap();
    assert_eq!(
        fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    file.retire().unwrap();
    let unrelated = fixture.0.join("unrelated");
    fs::write(&unrelated, b"preserve").unwrap();
    symlink(&unrelated, &file.path).unwrap();
    assert!(matches!(
        PendingFile::open(&config),
        Err(StorageError::InvalidPath)
    ));
    assert_eq!(file.retire(), Err(StorageError::InvalidPath));
    assert_eq!(fs::read(&unrelated).unwrap(), b"preserve");
    assert!(
        fs::symlink_metadata(&file.path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
