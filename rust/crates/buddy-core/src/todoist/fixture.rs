//! An explicit disposable desktop fixture cannot use real credentials or an
//! upstream origin. Ordinary Client::new never consults this marker.
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Timelike, Utc};
use reqwest::Url;
use serde::Deserialize;
use uuid::Uuid;

use super::TransportError;

const TOKEN: &str = "native-fixture-todoist-token";
const MAX_MARKER: u64 = 4096;
const MARKER: &str = ".buddy-todoist-fixture.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u8,
    nonce: Uuid,
    api_url: String,
    now_utc: String,
}

#[derive(Debug, PartialEq)]
pub(super) struct Connection {
    pub api: Url,
    pub now: DateTime<Utc>,
}

struct Host {
    github_actions: Option<std::ffi::OsString>,
    enabled: Option<std::ffi::OsString>,
    runner_temp: Option<std::ffi::OsString>,
}
pub(super) fn load(token: &str, config: &Path) -> Result<Connection, TransportError> {
    validate(
        token,
        config,
        &Host {
            github_actions: std::env::var_os("GITHUB_ACTIONS"),
            enabled: std::env::var_os("BUDDY_TODOIST_FIXTURE"),
            runner_temp: std::env::var_os("RUNNER_TEMP"),
        },
    )
}
fn regular(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}
fn validate(token: &str, config: &Path, host: &Host) -> Result<Connection, TransportError> {
    let unavailable = TransportError::FixtureUnavailable;
    // These checks precede filesystem access. No owner token or profile lookup.
    if token != TOKEN
        || host.github_actions.as_deref() != Some(std::ffi::OsStr::new("true"))
        || host.enabled.as_deref() != Some(std::ffi::OsStr::new("1"))
        || !config.is_absolute()
    {
        return Err(unavailable);
    }
    let root = host
        .runner_temp
        .as_ref()
        .map(PathBuf::from)
        .ok_or(unavailable)?;
    if !root.is_absolute() || root.is_symlink() || !root.is_dir() || !regular(config) {
        return Err(unavailable);
    }
    let root = root.canonicalize().map_err(|_| unavailable)?;
    let config = config.canonicalize().map_err(|_| unavailable)?;
    if !config.starts_with(&root) || config == root {
        return Err(unavailable);
    }
    let marker = config.parent().ok_or(unavailable)?.join(MARKER);
    if !regular(&marker) {
        return Err(unavailable);
    }
    let file = std::fs::File::open(marker).map_err(|_| unavailable)?;
    let mut bytes = Vec::new();
    file.take(MAX_MARKER + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    if bytes.len() as u64 > MAX_MARKER {
        return Err(unavailable);
    }
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| unavailable)?;
    if marker.version != 1 || marker.nonce.get_version_num() != 4 {
        return Err(unavailable);
    }
    let url = Url::parse(&marker.api_url).map_err(|_| unavailable)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/api/v1/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(unavailable);
    }
    let now = DateTime::parse_from_rfc3339(&marker.now_utc)
        .map_err(|_| unavailable)?
        .with_timezone(&Utc);
    if !(1..=9999).contains(&now.year()) || now.nanosecond() >= 1_000_000_000 {
        return Err(unavailable);
    }
    Ok(Connection { api: url, now })
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
