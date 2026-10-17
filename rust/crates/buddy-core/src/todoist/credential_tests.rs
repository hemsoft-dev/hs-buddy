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
