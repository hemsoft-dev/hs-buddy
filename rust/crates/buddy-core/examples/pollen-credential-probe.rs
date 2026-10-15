//! Disposable-CI interop probe. Credential values travel only through stdin.
use std::io::Read as _;
use std::path::PathBuf;

use buddy_core::pollen_credentials::{self, SERVICE};

fn run() -> Result<(), &'static str> {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().ok_or("Missing probe mode")?;
    let path = PathBuf::from(arguments.next().ok_or("Missing fixture profile")?);
    let marker = path
        .parent()
        .ok_or("Invalid fixture profile")?
        .join(".buddy-credential-fixture");
    if std::fs::read_to_string(marker).ok().as_deref() != Some("buddy credential fixture v1") {
        return Err("Only an owned fixture profile is permitted");
    }
    if mode == "account" {
        println!(
            "{}",
            pollen_credentials::account_for_path(&path).map_err(|_| "Invalid profile")?
        );
        return Ok(());
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("BUDDY_CREDENTIAL_FIXTURE").as_deref() != Ok("1")
    {
        return Err("OS credential probing is restricted to disposable GitHub Actions fixtures");
    }
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .map_err(|_| "Could not read probe input")?;
    match mode.as_str() {
        "read" => {
            let stored = pollen_credentials::load_at(&path).map_err(|_| "Secure read failed")?;
            if stored.as_deref() != Some(value.as_str()) {
                return Err("Cross-process credential mismatch");
            }
        }
        "read-none" => {
            if pollen_credentials::load_at(&path)
                .map_err(|_| "Secure read failed")?
                .is_some()
            {
                return Err("Expected no configured credential");
            }
        }
        "write" => pollen_credentials::save_at(&path, &value).map_err(|_| "Secure write failed")?,
        "clear" => pollen_credentials::save_at(&path, "").map_err(|_| "Secure clear failed")?,
        "delete" => {
            let account =
                pollen_credentials::account_for_path(&path).map_err(|_| "Invalid profile")?;
            let entry = keyring::Entry::new(SERVICE, &account)
                .map_err(|_| "Could not open fixture entry")?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(_) => return Err("Fixture credential cleanup failed"),
            }
        }
        _ => return Err("Unknown probe mode"),
    }
    println!("ok");
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Credential fixture failed: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}
