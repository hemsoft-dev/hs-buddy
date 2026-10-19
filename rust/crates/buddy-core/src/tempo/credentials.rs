//! One OS-vault entry holds a complete Tempo/Jira connection for a config
//! profile. No environment or other service's credential entry is consulted.
use std::path::Path;

use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "hs-buddy-tempo-v1";
const MAX_ENTRY_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    Invalid,
    Profile,
    Unavailable,
    Unconfirmed,
    Recovery(super::StorageError),
}
impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Recovery(error) => return error.fmt(f),
            Self::Invalid => "Enter valid Tempo and Jira credentials with a bare Jira HTTPS origin.",
            Self::Profile => "The configuration profile is unavailable. Choose the original profile before reconnecting.",
            Self::Unavailable => "Secure storage is unavailable. Unlock your OS credential store and try again.",
            Self::Unconfirmed => "Secure storage could not confirm the whole Tempo connection. Check it before continuing.",
        })
    }
}
impl std::error::Error for CredentialError {}

/// No Debug or Serialize. The only encoded representation belongs in the vault.
pub struct Credentials {
    tempo_origin: super::ApiOrigin,
    tempo_token: String,
    jira_origin: String,
    jira_email: String,
    jira_token: String,
}
impl Credentials {
    pub fn new(
        tempo_token: &str,
        jira_origin: &str,
        jira_email: &str,
        jira_token: &str,
    ) -> Result<Self, CredentialError> {
        Self::new_in_region(
            super::ApiOrigin::Global,
            tempo_token,
            jira_origin,
            jira_email,
            jira_token,
        )
    }
    pub fn new_in_region(
        tempo_origin: super::ApiOrigin,
        tempo_token: &str,
        jira_origin: &str,
        jira_email: &str,
        jira_token: &str,
    ) -> Result<Self, CredentialError> {
        if !super::valid_token(tempo_token) {
            return Err(CredentialError::Invalid);
        }
        let origin = super::jira::connection_origin(jira_origin, jira_email, jira_token)
            .map_err(|_| CredentialError::Invalid)?;
        Ok(Self {
            tempo_origin,
            tempo_token: tempo_token.into(),
            jira_origin: origin.into(),
            jira_email: jira_email.into(),
            jira_token: jira_token.into(),
        })
    }
    pub fn tempo_token(&self) -> &str {
        &self.tempo_token
    }
    pub fn tempo_origin(&self) -> super::ApiOrigin {
        self.tempo_origin
    }
    pub fn jira_origin(&self) -> &str {
        &self.jira_origin
    }
    pub fn jira_email(&self) -> &str {
        &self.jira_email
    }
    pub fn jira_token(&self) -> &str {
        &self.jira_token
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    #[serde(default)]
    tempo_origin: super::ApiOrigin,
    tempo_token: String,
    jira_origin: String,
    jira_email: String,
    jira_token: String,
}
fn encode(value: &Credentials) -> Result<String, CredentialError> {
    serde_json::to_string(&Stored {
        version: 1,
        tempo_origin: value.tempo_origin,
        tempo_token: value.tempo_token.clone(),
        jira_origin: value.jira_origin.clone(),
        jira_email: value.jira_email.clone(),
        jira_token: value.jira_token.clone(),
    })
    .map_err(|_| CredentialError::Invalid)
}
fn decode(value: &str) -> Result<Credentials, CredentialError> {
    if value.len() > MAX_ENTRY_BYTES {
        return Err(CredentialError::Invalid);
    }
    let stored: Stored = serde_json::from_str(value).map_err(|_| CredentialError::Invalid)?;
    if stored.version != 1 {
        return Err(CredentialError::Invalid);
    }
    Credentials::new_in_region(
        stored.tempo_origin,
        &stored.tempo_token,
        &stored.jira_origin,
        &stored.jira_email,
        &stored.jira_token,
    )
}
fn account(path: &Path) -> Result<String, CredentialError> {
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
pub fn load_at(path: &Path) -> Result<Option<Credentials>, CredentialError> {
    load_with(&OsVault::new(path)?)
}
pub fn save_at(path: &Path, value: &Credentials) -> Result<(), CredentialError> {
    save_preserving_with(path, value, None, &OsVault::new(path)?)
}
/// Authenticate the candidate against this exact recovery record before saving.
/// Publication and protected-store write/readback share the recovery lease.
pub fn save_preserving_recovery_at(
    path: &Path,
    value: &Credentials,
    expected: Option<&[u8]>,
) -> Result<(), CredentialError> {
    save_preserving_with(path, value, expected, &OsVault::new(path)?)
}
pub fn clear_at(path: &Path) -> Result<(), CredentialError> {
    clear_if_empty_with(path, &OsVault::new(path)?)
}
fn pending_file(path: &Path) -> Result<crate::pending_file::PendingFile, CredentialError> {
    crate::pending_file::PendingFile::open(
        path,
        crate::pending_file::RecoverySlot::Tempo,
        super::recovery::MAX_STORED_BYTES,
    )
    .map_err(CredentialError::Recovery)
}
fn save_preserving_with(
    path: &Path,
    value: &Credentials,
    expected: Option<&[u8]>,
    vault: &impl Vault,
) -> Result<(), CredentialError> {
    pending_file(path)?
        .while_matching(expected, || save_with(vault, value))
        .map_err(CredentialError::Recovery)?
}
fn clear_if_empty_with(path: &Path, vault: &impl Vault) -> Result<(), CredentialError> {
    pending_file(path)?
        .while_empty(|| clear_with(vault))
        .map_err(CredentialError::Recovery)?
}
fn load_with(vault: &impl Vault) -> Result<Option<Credentials>, CredentialError> {
    vault
        .read()
        .map_err(|_| CredentialError::Unavailable)?
        .as_deref()
        .map(decode)
        .transpose()
}
fn save_with(vault: &impl Vault, value: &Credentials) -> Result<(), CredentialError> {
    let value = encode(value)?;
    vault
        .write(&value)
        .map_err(|_| CredentialError::Unconfirmed)?;
    match vault.read() {
        Ok(Some(stored)) if stored == value => Ok(()),
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
