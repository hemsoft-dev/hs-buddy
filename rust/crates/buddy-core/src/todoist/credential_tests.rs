use std::cell::{Cell, RefCell};

use super::*;

#[derive(Default)]
struct MemoryVault {
    value: RefCell<Option<String>>,
    reads_fail: Cell<bool>,
    writes_fail: Cell<bool>,
    deletes_fail: Cell<bool>,
    mismatched_read: Cell<bool>,
    writes: Cell<usize>,
}

struct RecoveryProfile(std::path::PathBuf);
impl RecoveryProfile {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buddy-todoist-credential-race-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> std::path::PathBuf {
        self.0.join("config.json")
    }
}
impl Drop for RecoveryProfile {
    fn drop(&mut self) {
        let path = self.0.canonicalize().unwrap();
        assert_eq!(
            path.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("buddy-todoist-credential-race-")
        );
        assert!(
            !std::fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn replacement_save_refuses_new_recovery_before_touching_the_old_credential() {
    let profile = RecoveryProfile::new();
    let mut file = super::super::storage::PendingFile::open(&profile.config()).unwrap();
    let vault = MemoryVault::default();
    *vault.value.borrow_mut() = Some("old-account-token".into());
    let bytes = b"another window's captured recovery";
    file.persist(bytes).unwrap();
    assert_eq!(
        save_preserving_with(&profile.config(), "replacement-token", None, &vault),
        Err(CredentialError::Recovery(
            super::super::storage::StorageError::Changed
        ))
    );
    assert_eq!(vault.writes.get(), 0);
    assert!(vault.value.borrow().as_deref() == Some("old-account-token"));
    assert!(
        super::super::storage::PendingFile::open(&profile.config())
            .unwrap()
            .current()
            == Some(bytes.as_slice())
    );
}

#[test]
fn replacement_save_requires_the_exact_authenticated_pending_snapshot() {
    let profile = RecoveryProfile::new();
    let mut file = super::super::storage::PendingFile::open(&profile.config()).unwrap();
    let bytes = b"authenticated original-account recovery";
    file.persist(bytes).unwrap();
    let vault = MemoryVault::default();
    save_preserving_with(
        &profile.config(),
        "same-account-replacement",
        Some(bytes),
        &vault,
    )
    .unwrap();
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(
        save_preserving_with(
            &profile.config(),
            "foreign-token",
            Some(b"changed snapshot"),
            &vault
        ),
        Err(CredentialError::Recovery(
            super::super::storage::StorageError::Changed
        ))
    );
    assert_eq!(vault.writes.get(), 1);
    assert!(vault.value.borrow().as_deref() == Some("same-account-replacement"));
    assert!(
        super::super::storage::PendingFile::open(&profile.config())
            .unwrap()
            .current()
            == Some(bytes.as_slice())
    );
}

#[test]
fn replacement_save_holds_the_recovery_lock_through_actual_vault_write_and_readback() {
    use std::sync::mpsc;
    struct HeldVault {
        value: RefCell<Option<String>>,
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl Vault for HeldVault {
        fn read(&self) -> Result<Option<String>, ()> {
            Ok(self.value.borrow().clone())
        }
        fn write(&self, value: &str) -> Result<(), ()> {
            *self.value.borrow_mut() = Some(value.into());
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
        fn delete(&self) -> Result<(), ()> {
            panic!("Replacement must not remove credentials")
        }
    }
    let profile = RecoveryProfile::new();
    let config = profile.config();
    let mut publisher = super::super::storage::PendingFile::open(&config).unwrap();
    let (entered, writing) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let vault = HeldVault {
            value: RefCell::new(None),
            entered,
            release: released,
        };
        save_preserving_with(&config, "replacement-token", None, &vault)
    });
    writing
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        publisher.persist(b"new recovery"),
        Err(super::super::storage::StorageError::Busy)
    );
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), Ok(()));
    publisher.persist(b"new recovery").unwrap();
}
impl Vault for MemoryVault {
    fn read(&self) -> Result<Option<String>, ()> {
        if self.reads_fail.get() {
            return Err(());
        }
        if self.mismatched_read.get() {
            return Ok(Some("another-synthetic-token".into()));
        }
        Ok(self.value.borrow().clone())
    }
    fn write(&self, value: &str) -> Result<(), ()> {
        self.writes.set(self.writes.get() + 1);
        *self.value.borrow_mut() = Some(value.into());
        if self.writes_fail.get() {
            Err(())
        } else {
            Ok(())
        }
    }
    fn delete(&self) -> Result<(), ()> {
        self.value.borrow_mut().take();
        if self.deletes_fail.get() {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[test]
fn independent_config_profiles_have_independent_vault_accounts_without_a_global_fallback() {
    let root = std::env::temp_dir().join("buddy-synthetic-todoist-vault");
    assert_ne!(
        account(&root.join("a/config.json")).unwrap(),
        account(&root.join("b/config.json")).unwrap()
    );
    assert_eq!(
        account(&root.join("a/config.json")).unwrap(),
        account(&root.join("b/../a/config.json")).unwrap()
    );
    assert_ne!(SERVICE, crate::pollen_credentials::SERVICE);
}

#[test]
fn absent_and_unavailable_vaults_are_distinct_and_never_choose_another_key() {
    let vault = MemoryVault::default();
    assert!(load_with(&vault).unwrap().is_none());
    vault.reads_fail.set(true);
    assert!(matches!(
        load_with(&vault),
        Err(CredentialError::Unavailable)
    ));
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn invalid_input_is_rejected_before_any_vault_write() {
    let vault = MemoryVault::default();
    for value in ["", "private token", "private\nvalue"] {
        assert_eq!(save_with(&vault, value), Err(CredentialError::Invalid));
    }
    assert_eq!(
        save_with(&vault, &"x".repeat(4097)),
        Err(CredentialError::Invalid)
    );
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn saving_and_clearing_require_matching_readback() {
    let vault = MemoryVault::default();
    save_with(&vault, "synthetic-todoist-token").unwrap();
    assert!(load_with(&vault).unwrap().as_deref() == Some("synthetic-todoist-token"));
    clear_with(&vault).unwrap();
    assert!(load_with(&vault).unwrap().is_none());
}

#[test]
fn a_failed_or_mismatched_write_is_unconfirmed_even_if_the_vault_changed() {
    for mode in 0..3 {
        let vault = MemoryVault::default();
        match mode {
            0 => vault.writes_fail.set(true),
            1 => vault.reads_fail.set(true),
            _ => vault.mismatched_read.set(true),
        }
        assert_eq!(
            save_with(&vault, "synthetic-private-token"),
            Err(CredentialError::Unconfirmed)
        );
        assert!(vault.value.borrow().as_deref() == Some("synthetic-private-token"));
        assert!(
            !CredentialError::Unconfirmed
                .to_string()
                .contains("synthetic-private-token")
        );
    }
}

#[test]
fn clear_failure_or_retained_readback_is_not_reported_as_success() {
    for mode in 0..3 {
        let vault = MemoryVault::default();
        match mode {
            0 => vault.deletes_fail.set(true),
            1 => vault.reads_fail.set(true),
            _ => vault.mismatched_read.set(true),
        }
        assert_eq!(clear_with(&vault), Err(CredentialError::Unconfirmed));
    }
}

#[test]
fn invalid_stored_tokens_are_not_treated_as_an_empty_vault() {
    let vault = MemoryVault::default();
    *vault.value.borrow_mut() = Some("private\ninvalid".into());
    assert!(matches!(load_with(&vault), Err(CredentialError::Invalid)));
}
