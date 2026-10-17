//! Todoist tokens use the existing OS vault and config-profile identity.
//! Callers retain immutable clients and resolve uncertain writes before
//! replacing a connection. This module never falls back to an environment key.
use std::path::Path;

use keyring::Entry;

use super::valid_token;

const SERVICE: &str = "hs-buddy-todoist-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    Invalid,
    Profile,
    Unavailable,
    Unconfirmed,
    Recovery(super::storage::StorageError),
}
impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Recovery(error) => return error.fmt(f),
            Self::Invalid => "Enter a valid Todoist API token without spaces or control characters.",
            Self::Profile => "The configuration profile is unavailable. Choose the original profile before reconnecting.",
            Self::Unavailable => "Secure storage is unavailable. Unlock your OS credential store and try again.",
            Self::Unconfirmed => "Secure storage could not confirm this change. Check the connection before continuing.",
        })
    }
}
impl std::error::Error for CredentialError {}

fn account(path: &Path) -> Result<String, CredentialError> {
    // Reuse the established Windows/Unix path normalization. The account is
    // opaque; the distinct service prevents access to any pollen vault entry.
    crate::pollen_credentials::account_for_path(path).map_err(|_| CredentialError::Profile)
}
trait Vault {
    fn read(&self) -> Result<Option<String>, ()>;
    fn write(&self, value: &str) -> Result<(), ()>;
    fn delete(&self) -> Result<(), ()>;
}
struct OsVault(Entry);
impl OsVault {
    fn new(path: &Path) -> Result<Self, CredentialError> {
        Entry::new(SERVICE, &account(path)?)
            .map(Self)
            .map_err(|_| CredentialError::Unavailable)
    }
}
impl Vault for OsVault {
    fn read(&self) -> Result<Option<String>, ()> {
        match self.0.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(()),
        }
    }
    fn write(&self, value: &str) -> Result<(), ()> {
        self.0.set_password(value).map_err(|_| ())
    }
    fn delete(&self) -> Result<(), ()> {
        match self.0.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(()),
        }
    }
}
pub fn load_at(path: &Path) -> Result<Option<String>, CredentialError> {
    load_with(&OsVault::new(path)?)
}
pub fn save_at(path: &Path, token: &str) -> Result<(), CredentialError> {
    if !valid_token(token) {
        return Err(CredentialError::Invalid);
    }
    save_with(&OsVault::new(path)?, token)
}
pub fn clear_at(path: &Path) -> Result<(), CredentialError> {
    clear_with(&OsVault::new(path)?)
}

/// Check absence and delete under the same recovery-publication lock.
pub fn clear_if_no_pending_at(path: &Path) -> Result<(), CredentialError> {
    let file = super::storage::PendingFile::open(path).map_err(CredentialError::Recovery)?;
    file.while_empty(|| clear_at(path))
        .map_err(CredentialError::Recovery)?
}
fn load_with(vault: &impl Vault) -> Result<Option<String>, CredentialError> {
    let value = vault.read().map_err(|_| CredentialError::Unavailable)?;
    if value.as_deref().is_some_and(|token| !valid_token(token)) {
        return Err(CredentialError::Invalid);
    }
    Ok(value)
}
fn save_with(vault: &impl Vault, token: &str) -> Result<(), CredentialError> {
    if !valid_token(token) {
        return Err(CredentialError::Invalid);
    }
    vault
        .write(token)
        .map_err(|_| CredentialError::Unconfirmed)?;
    match vault.read() {
        Ok(Some(stored)) if stored == token => Ok(()),
        _ => Err(CredentialError::Unconfirmed),
    }
}
fn clear_with(vault: &impl Vault) -> Result<(), CredentialError> {
    vault.delete().map_err(|_| CredentialError::Unconfirmed)?;
    match vault.read() {
        Ok(None) => Ok(()),
        _ => Err(CredentialError::Unconfirmed),
    }
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
