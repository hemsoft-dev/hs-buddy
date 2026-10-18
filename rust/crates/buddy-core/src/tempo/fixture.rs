//! Explicit disposable CI transport only. Normal HTTPS constructors never read
//! this marker, and real credentials cannot enter either loopback client.
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Timelike, Utc};
use reqwest::Url;
use serde::Deserialize;
use uuid::Uuid;

use super::{Client, JiraClient, credentials::Credentials};

pub const TEMPO_TOKEN: &str = "native-fixture-tempo-token";
pub const JIRA_ORIGIN: &str = "https://native-tempo-jira.invalid/";
pub const JIRA_EMAIL: &str = "native-tempo-fixture@example.invalid";
pub const JIRA_TOKEN: &str = "native-fixture-jira-token";
const MARKER: &str = ".buddy-tempo-fixture.json";
const MAX_MARKER: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The disposable Tempo/Jira fixture is unavailable. Keep its isolated profile and check CI setup.")
    }
}
impl std::error::Error for Unavailable {}

/// No Debug: clients contain captured credentials. The protected credential
/// record retains its validated logical HTTPS Jira origin; only this explicit
/// CI connection uses the owned loopback routes.
pub struct Connection {
    pub tempo: Client,
    pub jira: JiraClient,
    pub now: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u8,
    nonce: String,
    tempo_url: String,
    jira_url: String,
    now_utc: String,
}
struct Host {
    actions: Option<std::ffi::OsString>,
    enabled: Option<std::ffi::OsString>,
    root: Option<std::ffi::OsString>,
}
struct Routes {
    tempo: Url,
    jira: Url,
    now: DateTime<Utc>,
}

pub fn connect(credentials: &Credentials, config: &Path) -> Result<Connection, Unavailable> {
    connect_with_host(
        credentials,
        config,
        &Host {
            actions: std::env::var_os("GITHUB_ACTIONS"),
            enabled: std::env::var_os("BUDDY_TEMPO_FIXTURE"),
            root: std::env::var_os("RUNNER_TEMP"),
        },
    )
}
fn connect_with_host(
    credentials: &Credentials,
    config: &Path,
    host: &Host,
) -> Result<Connection, Unavailable> {
    let routes = validate(credentials, config, host)?;
    let tempo = Client::build(TEMPO_TOKEN, routes.tempo, false).map_err(|_| Unavailable)?;
    let jira =
        JiraClient::build(routes.jira, JIRA_EMAIL, JIRA_TOKEN, false).map_err(|_| Unavailable)?;
    Ok(Connection {
        tempo,
        jira,
        now: routes.now,
    })
}
fn owned_kind(path: &Path, directory: bool) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
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
fn loopback(value: &str, path: &str) -> Result<Url, Unavailable> {
    let url = Url::parse(value).map_err(|_| Unavailable)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != path
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Unavailable);
    }
    Ok(url)
}
fn validate(credentials: &Credentials, config: &Path, host: &Host) -> Result<Routes, Unavailable> {
    // Exact public synthetic values are required before any filesystem access.
    if credentials.tempo_token() != TEMPO_TOKEN
        || credentials.jira_origin() != JIRA_ORIGIN
        || credentials.jira_email() != JIRA_EMAIL
        || credentials.jira_token() != JIRA_TOKEN
        || host.actions.as_deref() != Some(std::ffi::OsStr::new("true"))
        || host.enabled.as_deref() != Some(std::ffi::OsStr::new("1"))
        || !config.is_absolute()
    {
        return Err(Unavailable);
    }
    let root = host.root.as_ref().map(PathBuf::from).ok_or(Unavailable)?;
    if !root.is_absolute() || !owned_kind(&root, true) || !owned_kind(config, false) {
        return Err(Unavailable);
    }
    let root = root.canonicalize().map_err(|_| Unavailable)?;
    let config = config.canonicalize().map_err(|_| Unavailable)?;
    if !config.starts_with(&root) || config == root {
        return Err(Unavailable);
    }
    let marker = config.parent().ok_or(Unavailable)?.join(MARKER);
    if !owned_kind(&marker, false) {
        return Err(Unavailable);
    }
    let file = std::fs::File::open(marker).map_err(|_| Unavailable)?;
    let mut bytes = Vec::new();
    file.take(MAX_MARKER + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Unavailable)?;
    if bytes.len() as u64 > MAX_MARKER {
        return Err(Unavailable);
    }
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| Unavailable)?;
    let nonce = Uuid::parse_str(&marker.nonce).map_err(|_| Unavailable)?;
    if marker.version != 1 || nonce.get_version_num() != 4 {
        return Err(Unavailable);
    }
    let now = DateTime::parse_from_rfc3339(&marker.now_utc)
        .map_err(|_| Unavailable)?
        .with_timezone(&Utc);
    if !(1..=9999).contains(&now.year()) || now.nanosecond() >= 1_000_000_000 {
        return Err(Unavailable);
    }
    Ok(Routes {
        tempo: loopback(&marker.tempo_url, "/4/")?,
        jira: loopback(&marker.jira_url, "/")?,
        now,
    })
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
