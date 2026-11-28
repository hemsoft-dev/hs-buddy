//! The Finance card's quote cache across restarts (`readCache` and
//! `writeCache` in `useFinance.ts`, localStorage `finance:cache`).
//!
//! Electron stores `{quotes, timestamp, version: 3}` after each successful
//! fetch and paints the card from it on mount while it is younger than 15
//! minutes. Native keeps the same document, plus the watchlist the quotes were
//! fetched for, in [`FILE_NAME`] in the app's own cache directory
//! ([`cache_dir`]): never beside the settings, so no cache operation can reach
//! a settings file. Electron's localStorage is not readable here, so an
//! Electron cache does not carry over.
//!
//! - **Written** only for a complete batch: one finite quote for every
//!   watchlist symbol, in watchlist order. Electron also stores a batch in
//!   which some symbols failed; native never records a partial batch, so a
//!   failure leaves the previous complete cache in place.
//! - **Atomically**: through a new uniquely named staging file in the cache
//!   directory, created exclusively, then a rename. It is owner-only on Unix;
//!   on Windows the directory under `%LOCALAPPDATA%` is already per user.
//! - **Never through a link**: a save or clear acts only when the cache's
//!   name is missing or a regular file, checked before every rename attempt
//!   and before the removal; a link, directory or FIFO there is left alone.
//! - **Never with the settings inside**: when the settings file's resolved
//!   folder is the cache directory or inside it, nothing is cached.
//! - **Read** only while younger than [`TTL_MS`], for exactly the watchlist
//!   configured now, and only from a regular file that is not a link (a FIFO
//!   is never opened in a way that can block). A missing, unreadable,
//!   oversized, corrupt or other-version file, or one stamped in the future, is
//!   ignored and never shown as current.
//! - **Ordered off the UI thread**: [`Writer::shared`] gives every dashboard
//!   of the process the same writer, which serializes saves and clears, drops
//!   one requested before a newer one that already ran, and remembers a clear
//!   for the rest of the session.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::QuoteData;
use crate::config::{ConfigError, config_path, strip_bom};

/// The cache file in [`cache_dir`].
pub const FILE_NAME: &str = "finance-quotes.json";
/// An isolated cache directory (smoke runs and tests), read like
/// `BUDDY_CONFIG_PATH`.
pub const CACHE_DIR_ENV: &str = "BUDDY_CACHE_DIR";
/// The app's folder under the platform cache root, as for its settings.
const APP_DIR: &str = "Buddy";
/// Every staging file of a save starts with this.
const STAGING_PREFIX: &str = ".finance-quotes.";
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
    #[error("the quote cache's name is not a regular file, so it was left alone")]
    NotRegular,
    #[error(transparent)]
    Write(#[from] ConfigError),
}

/// The app's cache directory: `BUDDY_CACHE_DIR`, else `Buddy` under
/// `%LOCALAPPDATA%` (Windows), `~/Library/Caches` (macOS), or an absolute
/// `$XDG_CACHE_HOME` or `~/.cache` (elsewhere).
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_DIR_ENV) {
        let dir = PathBuf::from(dir);
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    Some(platform_cache_root()?.join(APP_DIR))
}

fn platform_cache_root() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library").join("Caches"));
    }
    // The XDG spec says to ignore a relative value.
    match std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from) {
        Some(xdg) if xdg.is_absolute() => Some(xdg),
        _ => Some(home()?.join(".cache")),
    }
}

/// The cache file for the resolved settings and cache directory
/// ([`path_in`]).
pub fn path() -> Option<PathBuf> {
    path_in(&cache_dir()?, config_path().ok().as_deref())
}

/// [`FILE_NAME`] in `dir`, or `None` when the settings file `config` lies
/// inside `dir`, so nothing is cached. The folders are compared resolved: the
/// settings file's folder (never its final name, which may be a link) against
/// the cache directory, and the settings file itself where it exists.
pub fn path_in(dir: &Path, config: Option<&Path>) -> Option<PathBuf> {
    let inside = config.is_some_and(|config| {
        let dir_resolved = resolved(dir);
        let folder = config.parent().map(resolved);
        config.starts_with(dir)
            || folder.is_some_and(|folder| folder.starts_with(&dir_resolved))
            || config
                .canonicalize()
                .is_ok_and(|file| file.starts_with(&dir_resolved))
    });
    (!inside).then(|| dir.join(FILE_NAME))
}

/// `path` with its longest existing ancestor resolved (links followed) and
/// the rest appended, so a folder that does not exist yet still compares.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = current.canonicalize() {
            return rest.iter().rev().fold(real, |path, name| path.join(name));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether a save or clear may act at `path`: nothing there, or a regular
/// file. Never a link (whatever it points to), directory or FIFO.
fn may_replace(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.file_type().is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(err) => Err(err),
    }
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
/// (a new staging file, then a rename). Anything but one complete set is
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
    // A reader on Windows can make the rename fail briefly; each attempt
    // checks the destination again first.
    let mut attempts = 0;
    loop {
        let renamed = match may_replace(path) {
            Ok(true) => std::fs::rename(&tmp, path),
            Ok(false) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(SaveError::NotRegular);
            }
            Err(err) => Err(err),
        };
        match renamed {
            Ok(()) => return Ok(()),
            Err(_) if attempts < 10 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(write(err).into());
            }
        }
    }
}

/// A new staging file beside `path` holding `body`, owner-only on Unix. The
/// name carries the process id and a random nonce and is created exclusively,
/// so no existing entry is ever reused or removed. A failed write removes
/// only the file it created.
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

/// Delete the cache (Clear Cache). A missing file is already cleared; a name
/// that is not a regular file (a link included) is left alone and reported.
/// The removal unlinks the name itself and never follows a link.
pub fn remove(path: &Path) -> std::io::Result<()> {
    if !may_replace(path)? {
        return Err(std::io::Error::other(
            "it is not a regular file, so it was not removed",
        ));
    }
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

/// Orders the cache's saves and clears when they run off the UI thread.
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
    /// A clear was attempted this session.
    cleared: AtomicBool,
}

/// One writer per cache file for the whole process.
static SHARED: LazyLock<Mutex<HashMap<PathBuf, Writer>>> = LazyLock::new(Default::default);

impl Writer {
    /// The process's writer for the cache at `path`, so every dashboard (a
    /// macOS window reopened after the last one closed included) shares one
    /// order and one cleared state.
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
    /// and before every one requested earlier that has not run yet. Whatever
    /// it does, the session counts the cache as cleared ([`cleared`](Self::cleared)).
    pub fn clear(&self, path: &Path) -> std::io::Result<Outcome> {
        self.state.cleared.store(true, Ordering::SeqCst);
        self.apply(self.ticket(), || remove(path))
            .map_or(Ok(Outcome::Superseded), |removed| {
                removed.map(|()| Outcome::Applied)
            })
    }

    /// Whether a clear was attempted this session, so the saved quotes are not
    /// restored again until Buddy restarts.
    pub fn cleared(&self) -> bool {
        self.state.cleared.load(Ordering::SeqCst)
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
