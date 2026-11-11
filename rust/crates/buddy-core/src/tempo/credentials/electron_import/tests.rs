use super::*;
use std::cell::{Cell, RefCell};

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
        panic!("import must never remove source or unknown protected contents")
    }
}

fn source() -> Result<Credentials, Error> {
    candidate(|field| {
        Some(
            match field {
                Field::TempoToken => "synthetic-tempo",
                Field::JiraToken => "synthetic-jira",
                Field::Email => "synthetic@example.invalid",
            }
            .into(),
        )
    })
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

#[test]
fn source_resolution_matches_electron_machine_precedence_and_process_fallback() {
    assert_eq!(
        resolve(Some(" machine \r\n".into()), || panic!("machine wins")),
        Some("machine".into())
    );
    for machine in [None, Some(" \r\n".into())] {
        assert_eq!(
            resolve(machine, || Some("process".into())),
            Some("process".into())
        );
    }
    // Process values are not trimmed by Electron. Invalid token whitespace is
    // rejected by the complete credential validator, not silently repaired.
    assert_eq!(
        resolve(None, || Some(" process ".into())),
        Some(" process ".into())
    );
    assert!(resolve(None, || Some(String::new())).is_none());
    assert!(resolve(None, || None).is_none());
}

#[test]
fn complete_import_roundtrips_electron_endpoints_without_touching_the_profile() {
    let (_directory, path) = profile();
    let original = std::fs::read(&path).unwrap();
    let vault = MemoryVault::default();
    assert_eq!(import_with(&path, &vault, source), Ok(Outcome::Imported));
    let saved = load_with(&vault).unwrap().unwrap();
    assert_eq!(saved.tempo_origin(), crate::tempo::ApiOrigin::Global);
    assert_eq!(saved.jira_origin(), "https://relias.atlassian.net/");
    assert_eq!(saved.tempo_token(), "synthetic-tempo");
    assert_eq!(saved.jira_token(), "synthetic-jira");
    assert_eq!(saved.jira_email(), "synthetic@example.invalid");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(vault.writes.get(), 1);
}

#[test]
fn repeat_never_reads_source_or_replaces_explicit_native_regional_choice() {
    let (_directory, path) = profile();
    let native = Credentials::new_in_region(
        crate::tempo::ApiOrigin::Europe,
        "native-tempo",
        "https://native.invalid",
        "native@example.invalid",
        "native-jira",
    )
    .unwrap();
    let vault = MemoryVault::default();
    let encoded = super::super::encode(&native).unwrap();
    *vault.value.borrow_mut() = Some(encoded.clone());
    for _ in 0..2 {
        assert_eq!(
            import_with(&path, &vault, || panic!("native choice wins")),
            Ok(Outcome::KeptNative)
        );
    }
    assert_eq!(vault.value.borrow().as_deref(), Some(encoded.as_str()));
    assert_eq!(vault.writes.get(), 0);

    let imported = MemoryVault::default();
    import_with(&path, &imported, source).unwrap();
    assert_eq!(
        import_with(&path, &imported, || panic!("repeat must not reread source")),
        Ok(Outcome::KeptNative)
    );
    assert_eq!(imported.writes.get(), 1);
}

#[test]
fn every_partial_or_invalid_source_is_rejected_before_a_write() {
    let (_directory, path) = profile();
    for missing in 0..3 {
        let vault = MemoryVault::default();
        let result = import_with(&path, &vault, || {
            candidate(|field| {
                let index = match field {
                    Field::TempoToken => 0,
                    Field::JiraToken => 1,
                    Field::Email => 2,
                };
                (index != missing).then(|| "synthetic".into())
            })
        });
        assert_eq!(result, Err(Error::Missing));
        assert_eq!(vault.writes.get(), 0);
    }
    for invalid in ["private token", "private\nvalue", "", &"x".repeat(8193)] {
        let vault = MemoryVault::default();
        assert!(import_with(&path, &vault, || candidate(|_| Some(invalid.into()))).is_err());
        assert_eq!(vault.writes.get(), 0);
    }
}

#[test]
fn corrupt_or_unavailable_native_vault_never_falls_back_to_electron() {
    let (_directory, path) = profile();
    for corrupt in [false, true] {
        let vault = MemoryVault::default();
        if corrupt {
            *vault.value.borrow_mut() = Some("private-corrupt-entry".into());
        } else {
            vault.fail_read.set(true);
        }
        assert!(matches!(
            import_with(&path, &vault, || panic!("no source fallback")),
            Err(Error::Credential(_))
        ));
        assert_eq!(vault.writes.get(), 0);
        if corrupt {
            assert_eq!(
                vault.value.borrow().as_deref(),
                Some("private-corrupt-entry")
            );
        }
    }
}

#[test]
fn missing_corrupt_and_relative_profiles_never_read_source_or_write_credentials() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
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
        import_with(Path::new("config.json"), &vault, source),
        Err(Error::Profile)
    );
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn pending_or_concurrently_leased_recovery_blocks_import_without_touching_either() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    let mut file = pending_file(&path).unwrap();
    file.persist(b"synthetic-pending-worklog").unwrap();
    assert_eq!(
        import_with(&path, &vault, source),
        Err(Error::Credential(CredentialError::Recovery(
            crate::tempo::StorageError::Pending
        )))
    );
    assert_eq!(
        pending_file(&path).unwrap().current(),
        Some(b"synthetic-pending-worklog".as_slice())
    );
    let lease = file.lease_pending().unwrap();
    assert_eq!(
        import_with(&path, &vault, source),
        Err(Error::Credential(CredentialError::Recovery(
            crate::tempo::StorageError::Busy
        )))
    );
    lease.check().unwrap();
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn import_holds_the_replacement_lease_before_resolving_or_writing_source() {
    let (_directory, path) = profile();
    let vault = MemoryVault::default();
    assert_eq!(
        import_with(&path, &vault, || {
            assert!(matches!(
                pending_file(&path),
                Err(CredentialError::Recovery(crate::tempo::StorageError::Busy))
            ));
            source()
        }),
        Ok(Outcome::Imported)
    );
}

#[test]
fn unconfirmed_storage_never_reports_success_or_deletes_unknown_contents() {
    let (_directory, path) = profile();
    for fail_write in [false, true] {
        let vault = MemoryVault::default();
        vault.fail_write.set(fail_write);
        vault.fail_readback.set(!fail_write);
        assert_eq!(
            import_with(&path, &vault, source),
            Err(Error::Credential(CredentialError::Unconfirmed))
        );
        assert_eq!(vault.writes.get(), 1);
        assert!(
            vault.value.borrow().is_some(),
            "possibly persisted full entry must remain inspectable"
        );
    }
}

#[test]
fn diagnostics_cannot_include_source_profile_or_vault_values() {
    for error in [
        Error::Missing,
        Error::Profile,
        Error::Credential(CredentialError::Invalid),
        Error::Credential(CredentialError::Unconfirmed),
        Error::Credential(CredentialError::Unavailable),
    ] {
        let text = error.to_string();
        assert!(!text.contains("synthetic") && !text.contains("private"));
    }
}
