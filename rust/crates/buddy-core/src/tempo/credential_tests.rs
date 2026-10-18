use std::cell::{Cell, RefCell};

use super::*;

#[derive(Default)]
struct MemoryVault {
    value: RefCell<Option<String>>,
    fail_read: Cell<bool>,
    fail_write: Cell<bool>,
    fail_delete: Cell<bool>,
    mismatch: Cell<bool>,
    writes: Cell<usize>,
    deletes: Cell<usize>,
}
impl Vault for MemoryVault {
    fn read(&self) -> Result<Option<String>, ()> {
        if self.fail_read.get() {
            return Err(());
        }
        if self.mismatch.get() {
            return Ok(Some("synthetic-mismatch".into()));
        }
        Ok(self.value.borrow().clone())
    }
    fn write(&self, value: &str) -> Result<(), ()> {
        self.writes.set(self.writes.get() + 1);
        *self.value.borrow_mut() = Some(value.into());
        if self.fail_write.get() {
            Err(())
        } else {
            Ok(())
        }
    }
    fn delete(&self) -> Result<(), ()> {
        self.deletes.set(self.deletes.get() + 1);
        self.value.borrow_mut().take();
        if self.fail_delete.get() {
            Err(())
        } else {
            Ok(())
        }
    }
}
fn credentials() -> Credentials {
    Credentials::new(
        "synthetic-tempo",
        "https://synthetic.invalid",
        "synthetic@example.invalid",
        "synthetic-jira",
    )
    .unwrap()
}

#[test]
fn empty_recovery_allows_an_atomic_connection_save_and_confirmed_clear() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let vault = MemoryVault::default();
    save_preserving_with(&config, &credentials(), None, &vault).unwrap();
    assert_eq!(
        load_with(&vault).unwrap().unwrap().tempo_token(),
        "synthetic-tempo"
    );
    clear_if_empty_with(&config, &vault).unwrap();
    assert!(load_with(&vault).unwrap().is_none());
    assert!(pending_file(&config).unwrap().current().is_none());
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(vault.deletes.get(), 1);
}

#[test]
fn pending_or_changed_recovery_blocks_credentials_before_touching_the_previous_connection() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let vault = MemoryVault::default();
    save_with(&vault, &credentials()).unwrap();
    let original_credentials = vault.value.borrow().clone();
    let mut file = pending_file(&config).unwrap();
    file.persist(b"original pending bytes").unwrap();
    assert_eq!(
        clear_if_empty_with(&config, &vault),
        Err(CredentialError::Recovery(
            super::super::StorageError::Pending
        ))
    );
    assert_eq!(
        save_preserving_with(&config, &credentials(), None, &vault),
        Err(CredentialError::Recovery(
            super::super::StorageError::Changed
        ))
    );
    std::fs::write(file.path(), b"changed since candidate authentication").unwrap();
    assert_eq!(
        save_preserving_with(
            &config,
            &credentials(),
            Some(b"original pending bytes"),
            &vault
        ),
        Err(CredentialError::Recovery(
            super::super::StorageError::Changed
        ))
    );
    assert_eq!(vault.value.borrow().clone(), original_credentials);
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(vault.deletes.get(), 0);
    assert_eq!(
        pending_file(&config).unwrap().current(),
        Some(b"changed since candidate authentication".as_slice())
    );
}

#[test]
fn authenticated_exact_pending_replacement_keeps_the_original_recovery_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut file = pending_file(&config).unwrap();
    file.persist(b"exact captured recovery").unwrap();
    let vault = MemoryVault::default();
    save_preserving_with(
        &config,
        &credentials(),
        Some(b"exact captured recovery"),
        &vault,
    )
    .unwrap();
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(
        load_with(&vault).unwrap().unwrap().jira_origin(),
        "https://synthetic.invalid/"
    );
    assert_eq!(
        pending_file(&config).unwrap().current(),
        Some(b"exact captured recovery".as_slice())
    );
}

#[test]
fn an_in_flight_pending_lease_blocks_credential_removal_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut file = pending_file(&config).unwrap();
    file.persist(b"a dispatched operation").unwrap();
    let lease = file.lease_pending().unwrap();
    let vault = MemoryVault::default();
    assert_eq!(
        clear_if_empty_with(&config, &vault),
        Err(CredentialError::Recovery(super::super::StorageError::Busy))
    );
    assert_eq!(
        save_preserving_with(
            &config,
            &credentials(),
            Some(b"a dispatched operation"),
            &vault
        ),
        Err(CredentialError::Recovery(super::super::StorageError::Busy))
    );
    assert_eq!(vault.writes.get(), 0);
    assert_eq!(vault.deletes.get(), 0);
    lease.check().unwrap();
}

struct LockCheckingVault {
    config: std::path::PathBuf,
    inner: MemoryVault,
    checks: Cell<usize>,
}
impl LockCheckingVault {
    fn check(&self) {
        assert!(matches!(
            pending_file(&self.config),
            Err(CredentialError::Recovery(super::super::StorageError::Busy))
        ));
        self.checks.set(self.checks.get() + 1);
    }
}
impl Vault for LockCheckingVault {
    fn read(&self) -> Result<Option<String>, ()> {
        self.check();
        self.inner.read()
    }
    fn write(&self, value: &str) -> Result<(), ()> {
        self.check();
        self.inner.write(value)
    }
    fn delete(&self) -> Result<(), ()> {
        self.check();
        self.inner.delete()
    }
}

#[test]
fn another_publisher_stays_blocked_during_actual_vault_write_readback_and_delete() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let vault = LockCheckingVault {
        config: config.clone(),
        inner: MemoryVault::default(),
        checks: Cell::new(0),
    };
    save_preserving_with(&config, &credentials(), None, &vault).unwrap();
    assert_eq!(vault.checks.get(), 2);
    clear_if_empty_with(&config, &vault).unwrap();
    assert_eq!(vault.checks.get(), 4);
    assert!(pending_file(&config).unwrap().current().is_none());
    assert_eq!(vault.inner.writes.get(), 1);
    assert_eq!(vault.inner.deletes.get(), 1);
}

#[test]
fn one_entry_roundtrips_the_whole_canonical_connection_and_clear_confirms_absence() {
    let vault = MemoryVault::default();
    save_with(&vault, &credentials()).unwrap();
    let value = load_with(&vault).unwrap().unwrap();
    assert_eq!(value.tempo_token(), "synthetic-tempo");
    assert_eq!(value.jira_origin(), "https://synthetic.invalid/");
    assert_eq!(value.jira_email(), "synthetic@example.invalid");
    assert_eq!(value.jira_token(), "synthetic-jira");
    assert_eq!(vault.writes.get(), 1);
    clear_with(&vault).unwrap();
    assert!(load_with(&vault).unwrap().is_none());
}

#[test]
fn invalid_connections_never_become_storable_credentials() {
    for (tempo, origin, email, jira) in [
        ("", "https://synthetic.invalid", "email", "token"),
        (
            "private token",
            "https://synthetic.invalid",
            "email",
            "token",
        ),
        ("token", "http://synthetic.invalid", "email", "token"),
        (
            "token",
            "https://user:secret@synthetic.invalid",
            "email",
            "token",
        ),
        (
            "token",
            "https://synthetic.invalid/private",
            "email",
            "token",
        ),
        (
            "token",
            "https://synthetic.invalid/?private=value",
            "email",
            "token",
        ),
        ("token", "https://synthetic.invalid", "", "token"),
        (
            "token",
            "https://synthetic.invalid",
            "email:password",
            "token",
        ),
        (
            "token",
            "https://synthetic.invalid",
            "email",
            "private\nkey",
        ),
    ] {
        assert!(matches!(
            Credentials::new(tempo, origin, email, jira),
            Err(CredentialError::Invalid)
        ));
    }
    assert!(
        Credentials::new(
            &"x".repeat(4097),
            "https://synthetic.invalid",
            "email",
            "token"
        )
        .is_err()
    );
    assert!(
        Credentials::new(
            "token",
            "https://synthetic.invalid",
            "email",
            &"x".repeat(8193)
        )
        .is_err()
    );
}

#[test]
fn invalid_or_partial_entries_are_not_reported_as_an_absent_connection() {
    let vault = MemoryVault::default();
    let original: serde_json::Value =
        serde_json::from_str(&encode(&credentials()).unwrap()).unwrap();
    let mut wrong_version = original.clone();
    wrong_version["version"] = 2.into();
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("jira_token");
    let mut unknown = original;
    unknown["other"] = true.into();
    for value in [
        "not-json".into(),
        wrong_version.to_string(),
        missing.to_string(),
        unknown.to_string(),
        " ".repeat(MAX_ENTRY_BYTES + 1),
    ] {
        *vault.value.borrow_mut() = Some(value);
        assert!(matches!(load_with(&vault), Err(CredentialError::Invalid)));
    }
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn missing_and_unavailable_stores_are_distinct_without_a_fallback() {
    let vault = MemoryVault::default();
    assert!(load_with(&vault).unwrap().is_none());
    vault.fail_read.set(true);
    assert!(matches!(
        load_with(&vault),
        Err(CredentialError::Unavailable)
    ));
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn failed_write_or_readback_keeps_the_whole_connection_unconfirmed_even_if_saved() {
    for mode in 0..3 {
        let vault = MemoryVault::default();
        match mode {
            0 => vault.fail_write.set(true),
            1 => vault.fail_read.set(true),
            _ => vault.mismatch.set(true),
        }
        assert_eq!(
            save_with(&vault, &credentials()),
            Err(CredentialError::Unconfirmed)
        );
        assert_eq!(
            vault.value.borrow().as_deref(),
            Some(encode(&credentials()).unwrap().as_str())
        );
        assert_eq!(vault.writes.get(), 1);
        assert!(
            !CredentialError::Unconfirmed
                .to_string()
                .contains("synthetic")
        );
    }
}

#[test]
fn failed_delete_or_retained_readback_never_confirms_disconnect() {
    for mode in 0..3 {
        let vault = MemoryVault::default();
        match mode {
            0 => vault.fail_delete.set(true),
            1 => vault.fail_read.set(true),
            _ => vault.mismatch.set(true),
        }
        assert_eq!(clear_with(&vault), Err(CredentialError::Unconfirmed));
    }
}

#[test]
fn profiles_use_distinct_keys_under_a_distinct_service_without_reading_other_vaults() {
    let path = std::env::temp_dir().join("synthetic-tempo-vault-profile");
    assert_ne!(
        account(&path.join("a/config.json")).unwrap(),
        account(&path.join("b/config.json")).unwrap()
    );
    assert_eq!(
        account(&path.join("a/config.json")).unwrap(),
        account(&path.join("b/../a/config.json")).unwrap()
    );
    assert_ne!(SERVICE, crate::pollen_credentials::SERVICE);
    assert_ne!(SERVICE, "hs-buddy-todoist-v1");
}
