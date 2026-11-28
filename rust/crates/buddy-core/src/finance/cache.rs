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

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::QuoteData;
use crate::config::{ConfigError, config_path, replace_with, strip_bom, write_private};

/// The cache file, beside `config.json`.
pub const FILE_NAME: &str = "finance-cache.json";
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

/// [`FILE_NAME`] next to the resolved `config.json`.
pub fn path() -> Result<PathBuf, ConfigError> {
    Ok(config_path()?.with_file_name(FILE_NAME))
}

/// The cached quotes when the file at `path` holds a complete set for
/// `symbols` fetched less than [`TTL_MS`] before `now_ms`; otherwise `None`.
pub fn load_fresh(path: &Path, symbols: &[String], now_ms: i64) -> Option<CachedQuotes> {
    if symbols.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    File::open(path)
        .ok()?
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
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    write_private(&tmp, &body).map_err(|source| {
        let _ = std::fs::remove_file(&tmp);
        write(source)
    })?;
    replace_with(&tmp, path).map_err(write)?;
    Ok(())
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
