//! The Finance card's quote cache across restarts (`readCache` and
//! `writeCache` in `useFinance.ts`, localStorage `finance:cache`).
//!
//! Electron stores `{quotes, timestamp, version: 3}` after each successful
//! fetch and paints the card from it on mount while it is younger than 15
//! minutes. Native keeps the same document, plus the watchlist the quotes were
//! fetched for, in [`FILE_NAME`] next to `config.json`. Electron's
//! localStorage is not readable here, so an Electron cache does not carry over.
//!
//! - **Written** only for a complete batch: one finite quote for every
//!   watchlist symbol, in watchlist order. Electron also stores a batch in
//!   which some symbols failed; native never records a partial batch, so a
//!   failure leaves the previous complete cache in place.
//! - **Read** only while younger than [`TTL_MS`], for exactly the watchlist
//!   configured now. A missing, unreadable, oversized, corrupt or
//!   other-version file, or one stamped in the future, is ignored and never
//!   shown as current.
//! - **Never the settings file**: [`path_beside`] picks a name that cannot be
//!   the resolved `config.json` or a name its writer derives (its staging file
//!   and lock directory), or no cache at all. An isolated `BUDDY_CONFIG_PATH`
//!   may itself be named [`FILE_NAME`], or that name may link to it. A link
//!   created there later is replaced or removed itself, never followed.
//! - **Its own staging file**: each save stages in a new
//!   `.finance-cache-staging.<pid>.<nonce>.tmp`, created exclusively, a name
//!   the settings writer's `<name>.json.<pid>.tmp` can never be, so a save
//!   never touches another writer's staging file.
//! - **Only a regular file is read**: a link, directory or FIFO at the cache's
//!   name is ignored without being opened for reading in a way that can block.
//! - **Ordered off the UI thread**: [`Writer::shared`] gives every dashboard
//!   of the process the same writer for a path, which serializes saves and
//!   removals and drops one requested before a newer one that already ran.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::QuoteData;
use crate::config::{ConfigError, config_path, replace_with, strip_bom};
use crate::config_lock::lock_dir;

/// The cache file, beside `config.json`.
pub const FILE_NAME: &str = "finance-cache.json";
/// The cache's name when [`FILE_NAME`] could be the settings file itself.
pub const FALLBACK_FILE_NAME: &str = "finance-cache.buddy.json";
/// Every staging file of a save starts with this; a settings file whose name
/// does is never written beside.
pub const STAGING_PREFIX: &str = ".finance-cache-staging.";
/// `CACHE_VERSION`.
pub const VERSION: u32 = 3;
/// `CACHE_TTL_MS`: 15 minutes.
pub const TTL_MS: i64 = 15 * 60 * 1000;
/// A file larger than this is ignored, and a cache that would exceed it is
/// not written.
pub const MAX_BYTES: u64 = 256 * 1024;

/// Quotes a cache held, with the epoch millisecond they were fetched at.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedQuotes {
    pub quotes: Vec<QuoteData>,
    pub fetched_at_ms: i64,
}

#[derive(Deserialize)]
struct StoredCache {
    version: u32,
    timestamp: i64,
    symbols: Vec<String>,
    quotes: Vec<QuoteData>,
}

#[derive(Serialize)]
struct CacheBody<'a> {
    quotes: &'a [QuoteData],
    timestamp: i64,
    version: u32,
    symbols: &'a [String],
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("the quotes are not one complete set for the watchlist")]
    Incomplete,
    #[error("the quote cache would exceed 256 KiB")]
    TooLarge,
    #[error(transparent)]
    Write(#[from] ConfigError),
}

/// The cache beside the resolved `config.json` ([`path_beside`]).
pub fn path() -> Result<Option<PathBuf>, ConfigError> {
    Ok(path_beside(&config_path()?))
}

/// [`FILE_NAME`] in the directory of `config`, or [`FALLBACK_FILE_NAME`] when
/// that could be one of the settings writer's files; `None` when both could,
/// or when the settings file is named like a cache staging file, so nothing
/// is cached.
pub fn path_beside(config: &Path) -> Option<PathBuf> {
    let staging_like = config.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(STAGING_PREFIX)
    });
    if staging_like {
        return None;
    }
    [FILE_NAME, FALLBACK_FILE_NAME]
        .into_iter()
        .map(|name| config.with_file_name(name))
        .find(|candidate| !may_collide(candidate, config))
}

/// Whether `candidate`, a name in the directory of `config`, could be a file
/// the settings writer uses: the settings file, its staging file
/// (`save_config_body_at`) or its lock directory, by name ignoring ASCII case
/// (Windows and macOS volumes usually ignore it), or the settings file once
/// both resolve (a link).
fn may_collide(candidate: &Path, config: &Path) -> bool {
    let staging = config.with_extension(format!("json.{}.tmp", std::process::id()));
    let same_name = |other: &Path| {
        candidate
            .file_name()
            .zip(other.file_name())
            .is_some_and(|(candidate, other)| candidate.eq_ignore_ascii_case(other))
    };
    [config, staging.as_path(), lock_dir(config).as_path()]
        .into_iter()
        .any(same_name)
        || matches!(
            (candidate.canonicalize(), config.canonicalize()),
            (Ok(candidate), Ok(config)) if candidate == config
        )
}

/// The file at `path` opened for reading, only when it is a regular file and
/// not a link. A FIFO is opened without blocking (and then refused).
fn open_regular(path: &Path) -> Option<File> {
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // The entry may be replaced between the check and the open.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// The cached quotes when the file at `path` holds a complete set for
/// `symbols` fetched less than [`TTL_MS`] before `now_ms`; otherwise `None`.
pub fn load_fresh(path: &Path, symbols: &[String], now_ms: i64) -> Option<CachedQuotes> {
    if symbols.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    open_regular(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() as u64 > MAX_BYTES {
        return None;
    }
    let stored: StoredCache =
        serde_json::from_str(strip_bom(std::str::from_utf8(&body).ok()?)).ok()?;
    let age = now_ms.checked_sub(stored.timestamp)?;
    let usable = stored.version == VERSION
        && (0..TTL_MS).contains(&age)
        && stored.symbols == symbols
        && is_complete(symbols, &stored.quotes);
    usable.then_some(CachedQuotes {
        quotes: stored.quotes,
        fetched_at_ms: stored.timestamp,
    })
}

/// Write `quotes` for `symbols`, fetched at `fetched_at_ms`, atomically
/// (owner-only temp file, then rename). Anything but one complete set is
/// refused before the file is touched.
pub fn save(
    path: &Path,
    symbols: &[String],
    quotes: &[QuoteData],
    fetched_at_ms: i64,
) -> Result<(), SaveError> {
    if !is_complete(symbols, quotes) {
        return Err(SaveError::Incomplete);
    }
    let body = serde_json::to_string(&CacheBody {
        quotes,
        timestamp: fetched_at_ms,
        version: VERSION,
        symbols,
    })
    .map_err(|_| SaveError::Incomplete)?;
    if body.len() as u64 > MAX_BYTES {
        return Err(SaveError::TooLarge);
    }
    let write = |source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(write)?;
    }
    let tmp = create_staging(path, &body).map_err(write)?;
    replace_with(&tmp, path).map_err(write)?;
    Ok(())
}

/// A new staging file beside `path` holding `body`, owner-only on Unix. The
/// name carries the process id and a random nonce and is created exclusively,
/// so no existing entry (another writer's staging file) is ever reused or
/// removed. A failed write removes only the file it created.
fn create_staging(path: &Path, body: &str) -> std::io::Result<PathBuf> {
    let mut attempts = 0;
    loop {
        let name = format!(
            "{STAGING_PREFIX}{}.n{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        let tmp = path.with_file_name(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = match options.open(&tmp) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempts < 3 => {
                attempts += 1;
                continue;
            }
            Err(err) => return Err(err),
        };
        if let Err(err) = file
            .write_all(body.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        return Ok(tmp);
    }
}

/// Delete the cache (Clear Cache). A missing file is already cleared.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// One quote per symbol, in order, with only finite numbers.
fn is_complete(symbols: &[String], quotes: &[QuoteData]) -> bool {
    !symbols.is_empty()
        && quotes.len() == symbols.len()
        && quotes.iter().zip(symbols).all(|(quote, symbol)| {
            quote.symbol == *symbol
                && [
                    quote.price,
                    quote.change,
                    quote.change_percent,
                    quote.previous_close,
                ]
                .iter()
                .all(|value| value.is_finite())
        })
}

/// Whether a [`Writer`] request ran or was dropped for a newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    Superseded,
}

/// Orders the cache's saves and removals when they run off the UI thread.
/// Each takes a [`ticket`](Self::ticket) when it is requested and runs later
/// with it. Runs are serialized, and one whose ticket is older than the newest
/// that already ran is dropped, so a slow earlier save can neither replace a
/// newer cache nor bring back a cleared one.
#[derive(Clone, Default)]
pub struct Writer {
    state: Arc<WriterState>,
}

#[derive(Default)]
struct WriterState {
    issued: AtomicU64,
    /// The newest ticket that ran; held while a request runs.
    applied: Mutex<u64>,
}

/// One writer per cache path for the whole process.
static SHARED: LazyLock<Mutex<HashMap<PathBuf, Writer>>> = LazyLock::new(Default::default);

impl Writer {
    /// The process's writer for `path`, so every dashboard (a macOS window
    /// reopened after the last one closed included) shares one order.
    pub fn shared(path: &Path) -> Self {
        SHARED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(path.to_path_buf())
            .or_default()
            .clone()
    }

    /// The next ticket, in request order.
    pub fn ticket(&self) -> u64 {
        self.state.issued.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// [`save`] unless a newer request already ran.
    pub fn save(
        &self,
        ticket: u64,
        path: &Path,
        symbols: &[String],
        quotes: &[QuoteData],
        fetched_at_ms: i64,
    ) -> Result<Outcome, SaveError> {
        self.apply(ticket, || save(path, symbols, quotes, fetched_at_ms))
            .map_or(Ok(Outcome::Superseded), |saved| {
                saved.map(|()| Outcome::Applied)
            })
    }

    /// [`remove`] now, with a new ticket: after any request already running,
    /// and before every one requested earlier that has not run yet.
    pub fn remove_next(&self, path: &Path) -> std::io::Result<Outcome> {
        self.remove(self.ticket(), path)
    }

    /// [`remove`] unless a newer request already ran.
    pub fn remove(&self, ticket: u64, path: &Path) -> std::io::Result<Outcome> {
        self.apply(ticket, || remove(path))
            .map_or(Ok(Outcome::Superseded), |removed| {
                removed.map(|()| Outcome::Applied)
            })
    }

    fn apply<T>(&self, ticket: u64, request: impl FnOnce() -> T) -> Option<T> {
        let mut applied = self
            .state
            .applied
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if ticket <= *applied {
            return None;
        }
        *applied = ticket;
        Some(request())
    }
}
