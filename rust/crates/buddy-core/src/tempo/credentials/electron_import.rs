//! Explicit, offline Windows import. Never called by normal connection loading.
//! Source values stay in this process and the existing profile-scoped OS vault.
use super::{CredentialError, Credentials, Vault, load_with, pending_file, save_with};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Imported,
    KeptNative,
}

#[derive(Debug, PartialEq, Eq)]
enum Error {
    Profile,
    Missing,
    Credential(CredentialError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Profile => f.write_str("Import requires an existing readable, valid Buddy configuration profile. No settings were changed."),
            Self::Missing => f.write_str("The complete Electron Tempo/Jira environment connection is unavailable. No credentials were saved."),
            Self::Credential(error) => error.fmt(f),
        }
    }
}

/// A closed allowlist; callers cannot turn the adapter into a registry dump.
#[derive(Clone, Copy)]
enum Field {
    TempoToken,
    JiraToken,
    Email,
}
impl Field {
    #[cfg(windows)]
    fn name(self) -> &'static str {
        match self {
            Self::TempoToken => "TEMPO_API_TOKEN",
            Self::JiraToken => "ATLASSIAN_API_TOKEN",
            Self::Email => "ATLASSIAN_EMAIL",
        }
    }
}

// Match src/utils/envLookup.ts: trim Machine, then use nonempty process verbatim.
fn resolve(machine: Option<String>, process: impl FnOnce() -> Option<String>) -> Option<String> {
    machine
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| process().filter(|value| !value.is_empty()))
}

fn candidate(mut source: impl FnMut(Field) -> Option<String>) -> Result<Credentials, Error> {
    let tempo = source(Field::TempoToken).ok_or(Error::Missing)?;
    let jira = source(Field::JiraToken).ok_or(Error::Missing)?;
    let email = source(Field::Email).ok_or(Error::Missing)?;
    // These are the actual Electron endpoints, not guessed native defaults.
    Credentials::new(&tempo, "https://relias.atlassian.net", &email, &jira)
        .map_err(Error::Credential)
}

fn import_with(
    path: &Path,
    vault: &impl Vault,
    source: impl FnOnce() -> Result<Credentials, Error>,
) -> Result<Outcome, Error> {
    // Loading defaults for a missing/corrupt profile would import to the wrong
    // identity. Read only, without config save or unrelated secret migrations.
    let valid_profile = path.is_absolute()
        && std::fs::read_to_string(path)
            .ok()
            .and_then(|body| crate::config::AppConfig::from_json(&body).ok())
            .is_some();
    if !valid_profile {
        return Err(Error::Profile);
    }
    pending_file(path)
        .map_err(Error::Credential)?
        .while_empty(|| {
            // This read and the complete write/readback share the same lease as
            // native Save/Forget/recovery publication. A concurrent native choice wins.
            if load_with(vault).map_err(Error::Credential)?.is_some() {
                return Ok(Outcome::KeptNative);
            }
            let value = source()?;
            // A failed write/readback is unconfirmed, as in native Save. Never
            // delete unknown contents on failure; existing Recheck/Forget can
            // inspect/remove a possibly persisted whole entry explicitly.
            save_with(vault, &value).map_err(Error::Credential)?;
            Ok(Outcome::Imported)
        })
        .map_err(|error| Error::Credential(CredentialError::Recovery(error)))?
}

#[cfg(windows)]
fn machine(field: Field) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW},
    };
    let key: Vec<_> = "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<_> = field.name().encode_utf16().chain(Some(0)).collect();
    // Windows environment variables have a 32767-character limit. Fixed bounded
    // storage avoids trusting a registry-provided allocation length.
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: terminated key/name, valid predefined read-only handle and a
    // writable aligned buffer with its full byte capacity. No secret subprocess.
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            // Electron uses Windows PowerShell's .NET Framework RegistryKey
            // GetValue, which expands REG_EXPAND_SZ using this process env.
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != ERROR_SUCCESS
        || bytes < 2
        || bytes as usize > buffer.len() * 2
        || !bytes.is_multiple_of(2)
    {
        return None;
    }
    let units = bytes as usize / 2;
    if buffer[units - 1] != 0 {
        return None;
    }
    String::from_utf16(&buffer[..units - 1]).ok()
}

/// Headless entry point, before app logging, telemetry, runtime or windows.
/// It makes no HTTP requests and emits only static outcomes, never source data.
#[cfg(windows)]
pub fn run() -> i32 {
    let result = crate::config::config_path()
        .map_err(|_| Error::Profile)
        .and_then(|path| {
            let vault = super::OsVault::new(&path).map_err(Error::Credential)?;
            import_with(&path, &vault, || {
                candidate(|field| resolve(machine(field), || std::env::var(field.name()).ok()))
            })
        });
    match result {
        Ok(Outcome::Imported) => {
            println!(
                "Electron Tempo/Jira connection imported into this profile's protected store. No service requests were sent."
            );
            0
        }
        Ok(Outcome::KeptNative) => {
            println!(
                "The existing native Tempo/Jira connection was kept. No credentials were changed."
            );
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
#[path = "electron_import/tests.rs"]
mod tests;
