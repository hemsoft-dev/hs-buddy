//! Explicit, offline import of Electron's Todoist token into an empty native
//! entry. Electron keeps no Todoist store of its own: it reads the
//! TODOIST_API_TOKEN environment variable, Machine scope first on Windows
//! (electron/services/todoistClient.ts, src/utils/envLookup.ts). Normal
//! connection loading never calls this, and the value stays in this process
//! and the existing profile-scoped OS vault.
use super::{CredentialError, Vault, load_with, save_with};
use std::path::Path;

/// The only name Electron's Todoist client reads.
const NAME: &str = "TODOIST_API_TOKEN";

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Imported,
    KeptNative,
}

/// No variant holds source data, so neither Display nor Debug can show it.
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
            Self::Missing => f.write_str("Electron's Todoist token (TODOIST_API_TOKEN) is unavailable to this process. No credentials were saved."),
            Self::Credential(error) => error.fmt(f),
        }
    }
}

fn import_with(
    path: &Path,
    vault: &impl Vault,
    source: impl FnOnce() -> Option<String>,
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
    let recovery = |error| Error::Credential(CredentialError::Recovery(error));
    super::super::storage::PendingFile::open(path)
        .map_err(recovery)?
        .while_empty(|| {
            // This read and the write/readback share the recovery lock that
            // native Save, Forget and recovery publication take. A native
            // token wins, so a repeat never brings an old Electron token back.
            if load_with(vault).map_err(Error::Credential)?.is_some() {
                return Ok(Outcome::KeptNative);
            }
            let token = source().ok_or(Error::Missing)?;
            // Validation runs before the write. A failed write/readback is
            // unconfirmed, as in native Save: unknown contents are never
            // deleted, and Recheck or Forget can inspect or remove them.
            save_with(vault, &token).map_err(Error::Credential)?;
            Ok(Outcome::Imported)
        })
        .map_err(recovery)?
}

/// Headless entry point, before app logging, telemetry, runtime or windows.
/// It makes no HTTP requests and prints only static outcomes, never the token.
pub fn run() -> i32 {
    let result = crate::config::config_path()
        .map_err(|_| Error::Profile)
        .and_then(|path| {
            let vault = super::OsVault::new(&path).map_err(Error::Credential)?;
            import_with(&path, &vault, || {
                crate::electron_env::electron_variable(NAME)
            })
        });
    match result {
        Ok(Outcome::Imported) => {
            println!(
                "Electron Todoist token imported into this profile's protected store. No service requests were sent."
            );
            0
        }
        Ok(Outcome::KeptNative) => {
            println!("The existing native Todoist token was kept. No credentials were changed.");
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
