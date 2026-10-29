//! Profile-scoped pollen credentials shared with Electron. New keys never
//! enter config JSON. Migration requires the existing config lock and a
//! verified vault write before clearing the exact legacy candidate.

use std::path::{Component, Path, PathBuf};

use keyring::Entry;

use crate::config::{AppConfig, save_config_body_at};
use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};

pub const SERVICE: &str = "hs-buddy-pollen-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("Secure storage is unavailable. Unlock your OS credential store and try again.")]
    Unavailable,
    #[error("Configuration is busy. Try again after the other Buddy instance finishes saving.")]
    Busy,
    #[error("The configuration could not be read or updated. Check the file and its permissions.")]
    Configuration,
    #[error(
        "The legacy key changed or differs from secure storage. Save your preferred key again to finish migration."
    )]
    Conflict,
    #[error("Enter an API key without control characters, up to 4096 bytes.")]
    Invalid,
    #[error("The configuration profile path is unavailable.")]
    Profile,
}

/// The same lexical absolute path normalization and SHA-256 are implemented
/// in Electron. Windows folds ASCII case and separators; Unix stays case
/// sensitive. A profile never falls back to a global owner's vault entry.
pub fn account_for_path(path: &Path) -> Result<String, CredentialError> {
    let absolute = std::path::absolute(path).map_err(|_| CredentialError::Profile)?;
    let mut normalized = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    let path = normalized.to_str().ok_or(CredentialError::Profile)?;
    Ok(account_for_normalized_path(path, cfg!(windows)))
}

fn account_for_normalized_path(path: &str, windows: bool) -> String {
    let path = if windows {
        let path = if let Some(tail) = path.strip_prefix("\\\\?\\UNC\\") {
            format!("//{tail}")
        } else {
            path.strip_prefix("\\\\?\\").unwrap_or(path).to_owned()
        };
        path.replace('\\', "/").to_ascii_lowercase()
    } else {
        path.to_owned()
    };
    format!("pollen-{}", crate::sha256::hex_digest(path.as_bytes()))
}

trait Vault {
    fn read(&self) -> Result<Option<String>, CredentialError>;
    fn write(&self, value: &str) -> Result<(), CredentialError>;
    fn delete(&self) -> Result<(), CredentialError>;
}

struct OsVault(Entry);

impl OsVault {
    fn new(path: &Path) -> Result<Self, CredentialError> {
        Entry::new(SERVICE, &account_for_path(path)?)
            .map(Self)
            .map_err(|_| CredentialError::Unavailable)
    }
}

impl Vault for OsVault {
    fn read(&self) -> Result<Option<String>, CredentialError> {
        match self.0.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(CredentialError::Unavailable),
        }
    }
    fn write(&self, value: &str) -> Result<(), CredentialError> {
        self.0
            .set_password(value)
            .map_err(|_| CredentialError::Unavailable)
    }
    fn delete(&self) -> Result<(), CredentialError> {
        match self.0.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredentialError::Unavailable),
        }
    }
}

pub fn load_at(path: &Path) -> Result<Option<String>, CredentialError> {
    read_with(path, &OsVault::new(path)?)
}

pub fn save_at(path: &Path, value: &str) -> Result<(), CredentialError> {
    save_with(path, &OsVault::new(path)?, value)
}

fn lock(path: &Path) -> Result<ConfigLock, CredentialError> {
    ConfigLock::acquire(path, DEFAULT_TIMEOUT)
        .map_err(|_| CredentialError::Busy)?
        .filter(ConfigLock::is_held)
        .ok_or(CredentialError::Busy)
}

fn legacy(path: &Path) -> Result<String, CredentialError> {
    AppConfig::load_from(path)
        .map(|config| config.ui.pollen_api_key)
        .map_err(|_| CredentialError::Configuration)
}

fn validate(value: &str) -> Result<(), CredentialError> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        Err(CredentialError::Invalid)
    } else {
        Ok(())
    }
}

fn clear_legacy(path: &Path, candidate: &str, held: &ConfigLock) -> Result<(), CredentialError> {
    if !held.is_held() {
        return Err(CredentialError::Busy);
    }
    let (mut config, stamp) =
        AppConfig::load_with_stamp_from(path).map_err(|_| CredentialError::Configuration)?;
    if config.ui.pollen_api_key != candidate {
        return Err(CredentialError::Conflict);
    }
    if candidate.is_empty() {
        return Ok(());
    }
    config.ui.pollen_api_key.clear();
    let body = serde_json::to_string_pretty(&config).map_err(|_| CredentialError::Configuration)?;
    if !held.is_held() {
        return Err(CredentialError::Busy);
    }
    if !save_config_body_at(path, &body, Some(&stamp))
        .map_err(|_| CredentialError::Configuration)?
    {
        return Err(CredentialError::Conflict);
    }
    Ok(())
}

fn read_with(path: &Path, vault: &impl Vault) -> Result<Option<String>, CredentialError> {
    let held = lock(path)?;
    let candidate = legacy(path)?;
    match vault.read()? {
        Some(value) => {
            // An empty protected entry is a clear tombstone. Keep it so a
            // failed JSON cleanup or stale old writer cannot restore a key.
            if !candidate.is_empty() && !value.is_empty() && value != candidate {
                return Err(CredentialError::Conflict);
            }
            clear_legacy(path, &candidate, &held)?;
            Ok((!value.is_empty()).then_some(value))
        }
        None if candidate.is_empty() => Ok(None),
        None => {
            validate(&candidate)?;
            if !held.is_held() {
                return Err(CredentialError::Busy);
            }
            vault.write(&candidate)?;
            if vault.read()?.as_deref() != Some(candidate.as_str()) {
                return Err(CredentialError::Unavailable);
            }
            if let Err(error) = clear_legacy(path, &candidate, &held) {
                // Undo only this migration's value under our surviving lock.
                // A newer vault value is never removed by this cleanup.
                if held.is_held()
                    && vault.read().ok().flatten().as_deref() == Some(candidate.as_str())
                {
                    let _ = vault.delete();
                }
                return Err(error);
            }
            Ok(Some(candidate))
        }
    }
}

fn save_with(path: &Path, vault: &impl Vault, value: &str) -> Result<(), CredentialError> {
    let value = value.trim();
    validate(value)?;
    let held = lock(path)?;
    let candidate = legacy(path)?;
    if !held.is_held() {
        return Err(CredentialError::Busy);
    }
    // The empty value deliberately clears the secret but retains the durable
    // tombstone. Neither a failed config write nor a stale legacy snapshot
    // may undo an explicit clear.
    vault.write(value)?;
    if vault.read()?.as_deref() != Some(value) {
        return Err(CredentialError::Unavailable);
    }
    clear_legacy(path, &candidate, &held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeVault {
        value: RefCell<Option<String>>,
        fail: bool,
        on_write: RefCell<Option<Box<dyn FnOnce()>>>,
    }
    impl Vault for FakeVault {
        fn read(&self) -> Result<Option<String>, CredentialError> {
            if self.fail {
                Err(CredentialError::Unavailable)
            } else {
                Ok(self.value.borrow().clone())
            }
        }
        fn write(&self, value: &str) -> Result<(), CredentialError> {
            if self.fail {
                return Err(CredentialError::Unavailable);
            }
            *self.value.borrow_mut() = Some(value.into());
            if let Some(callback) = self.on_write.borrow_mut().take() {
                callback();
            }
            Ok(())
        }
        fn delete(&self) -> Result<(), CredentialError> {
            self.value.borrow_mut().take();
            Ok(())
        }
    }

    fn fixture(label: &str, body: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("buddy-pollen-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn migration_verifies_vault_then_removes_only_legacy_key_and_preserves_unknowns() {
        let path = fixture(
            "migrate",
            r#"{"ui":{"pollenApiKey":"fixture-only-key","future":9},"future":10}"#,
        );
        let vault = FakeVault::default();
        assert_eq!(
            read_with(&path, &vault).unwrap().as_deref(),
            Some("fixture-only-key")
        );
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("fixture-only-key"));
        let saved = AppConfig::load_from(&path).unwrap();
        assert_eq!(saved.ui.extra["future"], 9);
        assert_eq!(saved.extra["future"], 10);
        assert!(saved.ui.pollen_api_key.is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn unavailable_vault_keeps_original_recoverable_bytes() {
        let body = r#"{"ui":{"pollenApiKey":"fixture-only-key"}}"#;
        let path = fixture("unavailable", body);
        let vault = FakeVault {
            fail: true,
            ..Default::default()
        };
        assert_eq!(read_with(&path, &vault), Err(CredentialError::Unavailable));
        assert_eq!(
            save_with(&path, &vault, "new-fixture-key"),
            Err(CredentialError::Unavailable)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        assert!(!CredentialError::Unavailable.to_string().contains("fixture"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn concurrent_legacy_edit_is_preserved_and_only_our_vault_migration_is_undone() {
        let path = fixture("conflict", r#"{"ui":{"pollenApiKey":"old-fixture"}}"#);
        let changed = path.clone();
        let body = r#"{"ui":{"pollenApiKey":"new-fixture"},"other":true}"#;
        let vault = FakeVault {
            on_write: RefCell::new(Some(Box::new(move || {
                std::fs::write(changed, body).unwrap()
            }))),
            ..Default::default()
        };
        assert_eq!(read_with(&path, &vault), Err(CredentialError::Conflict));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        assert_eq!(*vault.value.borrow(), None);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn explicit_clear_tombstone_prevents_legacy_resurrection() {
        let path = fixture("clear", r#"{"ui":{"pollenApiKey":"old-fixture"}}"#);
        let vault = FakeVault::default();
        save_with(&path, &vault, "").unwrap();
        std::fs::write(&path, r#"{"ui":{"pollenApiKey":"stale-fixture"}}"#).unwrap();
        assert_eq!(read_with(&path, &vault).unwrap(), None);
        assert_eq!(*vault.value.borrow(), Some(String::new()));
        assert!(
            AppConfig::load_from(&path)
                .unwrap()
                .ui
                .pollen_api_key
                .is_empty()
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn profiles_and_windows_case_variants_have_the_expected_identity() {
        assert_eq!(
            account_for_normalized_path("C:\\Users\\Test\\Buddy\\config.json", true),
            account_for_normalized_path("c:/users/test/buddy/config.json", true)
        );
        assert_eq!(
            account_for_normalized_path("\\\\?\\C:\\Users\\Test\\Buddy\\config.json", true),
            account_for_normalized_path("c:/users/test/buddy/config.json", true)
        );
        assert_ne!(
            account_for_normalized_path("/profiles/one/config.json", false),
            account_for_normalized_path("/profiles/two/config.json", false)
        );
        assert_ne!(
            account_for_normalized_path("/Profiles/config.json", false),
            account_for_normalized_path("/profiles/config.json", false)
        );
        let mut config = AppConfig::default();
        config.ui.pollen_api_key = "private-fixture-value".into();
        assert!(!format!("{config:?}").contains("private-fixture-value"));
    }
}
