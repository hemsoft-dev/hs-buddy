use super::*;
use std::cell::{Cell, RefCell};

/// A synthetic token with the shape of a real one. It must never surface.
const SOURCE: &str = "synthetic-electron-todoist-token";

#[derive(Default)]
struct MemoryVault {
    value: RefCell<Option<String>>,
    writes: Cell<usize>,
    fail_read: Cell<bool>,
    fail_write: Cell<bool>,
    fail_readback: Cell<bool>,
}
impl Vault for MemoryVault {
    fn read(&self) -> Result<Option<String>, ()> {
        if self.fail_read.get() || (self.fail_readback.get() && self.writes.get() > 0) {
            return Err(());
        }
        Ok(self.value.borrow().clone())
    }
    fn write(&self, value: &str) -> Result<(), ()> {
        self.writes.set(self.writes.get() + 1);
        // Simulate an OS adapter returning an error after the whole write.
        *self.value.borrow_mut() = Some(value.into());
        if self.fail_write.get() {
            Err(())
        } else {
            Ok(())
        }
    }
    fn delete(&self) -> Result<(), ()> {
        panic!("import must never remove a native or unknown protected entry")
    }
}

fn source() -> Option<String> {
    Some(SOURCE.into())
}
fn profile() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"electronUnknown":{"keep":true},"ui":{"theme":"light"}}"#,
    )
    .unwrap();
    (directory, path)
}
fn shown(result: &Result<Outcome, Error>) -> String {
    match result {
        Ok(outcome) => format!("{outcome:?}"),
        Err(error) => format!("{error} {error:?}"),
    }
}

#[test]
fn an_empty_native_entry_takes_electrons_token_once_without_touching_the_profile() {
    let (_directory, path) = profile();
    let original = std::fs::read(&path).unwrap();
    let vault = MemoryVault::default();
    assert_eq!(import_with(&path, &vault, source), Ok(Outcome::Imported));
    assert_eq!(load_with(&vault).unwrap().as_deref(), Some(SOURCE));
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    // Repeating it never reads Electron's value again, even a different one.
    for _ in 0..2 {
        assert_eq!(
            import_with(&path, &vault, || panic!(
                "a repeat must not reread the source"
            )),
            Ok(Outcome::KeptNative)
        );
    }
    assert_eq!(vault.writes.get(), 1);
    assert_eq!(load_with(&vault).unwrap().as_deref(), Some(SOURCE));
}

#[test]
fn an_existing_native_token_is_kept_and_the_source_is_never_read() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    *vault.value.borrow_mut() = Some("native-typed-token".into());
    assert_eq!(
        import_with(&path, &vault, || panic!("native choice wins")),
        Ok(Outcome::KeptNative)
    );
    assert_eq!(vault.value.borrow().as_deref(), Some("native-typed-token"));
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn a_missing_or_invalid_electron_token_is_rejected_before_any_write() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    assert_eq!(import_with(&path, &vault, || None), Err(Error::Missing));
    // Electron passes a process value verbatim; the native validator refuses
    // whitespace and control characters instead of repairing them.
    let long = "x".repeat(4097);
    for invalid in [
        " padded-token ",
        "private token",
        "private\nvalue",
        "\u{e9}t\u{e9}",
        long.as_str(),
    ] {
        assert_eq!(
            import_with(&path, &vault, || Some(invalid.into())),
            Err(Error::Credential(CredentialError::Invalid)),
            "{invalid:?}"
        );
    }
    assert_eq!(vault.writes.get(), 0);
    assert!(vault.value.borrow().is_none());
}

#[test]
fn a_corrupt_or_unavailable_native_entry_stops_import_without_a_write() {
    let (_directory, path) = profile();
    for corrupt in [false, true] {
        let vault = MemoryVault::default();
        if corrupt {
            *vault.value.borrow_mut() = Some("private corrupt entry".into());
        } else {
            vault.fail_read.set(true);
        }
        let expected = if corrupt {
            CredentialError::Invalid
        } else {
            CredentialError::Unavailable
        };
        assert_eq!(
            import_with(&path, &vault, || panic!("no source fallback")),
            Err(Error::Credential(expected))
        );
        assert_eq!(vault.writes.get(), 0);
        if corrupt {
            assert_eq!(
                vault.value.borrow().as_deref(),
                Some("private corrupt entry")
            );
        }
    }
}

#[test]
fn an_unconfirmed_write_is_reported_and_its_unknown_contents_are_kept() {
    let (_directory, path) = profile();
    for readback in [false, true] {
        let vault = MemoryVault::default();
        if readback {
            vault.fail_readback.set(true);
        } else {
            vault.fail_write.set(true);
        }
        assert_eq!(
            import_with(&path, &vault, source),
            Err(Error::Credential(CredentialError::Unconfirmed))
        );
        // The OS may hold the whole value. Recheck or Forget handles it; the
        // import never deletes (the vault panics on delete).
        assert_eq!(vault.writes.get(), 1);
    }
}

#[test]
fn missing_corrupt_or_relative_profiles_never_read_the_source_or_the_vault() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    vault.fail_read.set(true);
    for body in [None, Some("private-corrupt-config"), Some("[]")] {
        match body {
            Some(body) => std::fs::write(&path, body).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        assert_eq!(
            import_with(&path, &vault, || panic!("invalid profile")),
            Err(Error::Profile)
        );
        if let Some(body) = body {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        } else {
            assert!(!path.exists());
        }
    }
    assert_eq!(
        import_with(Path::new("config.json"), &vault, || panic!(
            "relative profile"
        )),
        Err(Error::Profile)
    );
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn a_pending_task_recovery_blocks_import_without_touching_either_store() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    let mut file = super::super::super::storage::PendingFile::open(&path).unwrap();
    file.persist(b"synthetic-pending-task-change").unwrap();
    assert_eq!(
        import_with(&path, &vault, || panic!("pending recovery wins")),
        Err(Error::Credential(CredentialError::Recovery(
            super::super::super::storage::StorageError::Pending
        )))
    );
    assert_eq!(
        super::super::super::storage::PendingFile::open(&path)
            .unwrap()
            .current(),
        Some(b"synthetic-pending-task-change".as_slice())
    );
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn no_outcome_or_error_ever_shows_the_token() {
    let (_directory, path) = profile();
    let mut results = Vec::new();
    results.push(import_with(&path, &MemoryVault::default(), source));
    let unconfirmed = MemoryVault::default();
    unconfirmed.fail_write.set(true);
    results.push(import_with(&path, &unconfirmed, source));
    let kept = MemoryVault::default();
    *kept.value.borrow_mut() = Some(SOURCE.into());
    results.push(import_with(&path, &kept, source));
    let invalid = format!("{SOURCE} with spaces");
    results.push(import_with(&path, &MemoryVault::default(), || {
        Some(invalid.clone())
    }));
    results.push(import_with(&path, &MemoryVault::default(), || None));
    results.push(import_with(
        Path::new("config.json"),
        &MemoryVault::default(),
        source,
    ));
    assert_eq!(results.len(), 6);
    for result in &results {
        let text = shown(result);
        assert!(
            !text.contains(SOURCE) && !text.contains("synthetic-electron"),
            "{text}"
        );
    }
}
