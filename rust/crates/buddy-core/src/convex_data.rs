//! Live Convex subscriptions for the dashboard (`buddyStats:get`,
//! `repoBookmarks:list`), forwarded over a channel to the UI runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;

use crate::stats::BuddyStats;

/// Local Convex dev backend (`npx convex dev` / Aspire), matching `.env.local`.
pub const DEFAULT_CONVEX_URL: &str = "http://127.0.0.1:3210";

/// Resolve the deployment URL the way the Electron app effectively does:
/// `BUDDY_CONVEX_URL`, then `VITE_CONVEX_URL` from the environment, then
/// `VITE_CONVEX_URL` from the Buddy checkout's `.env.local` or `.env`
/// (the checkout root found upward from the working directory or the
/// executable), then the local backend.
pub fn convex_url() -> String {
    for key in ["BUDDY_CONVEX_URL", "VITE_CONVEX_URL"] {
        if let Some(url) = std::env::var(key).ok().and_then(clean_url) {
            return url;
        }
    }
    for dir in search_roots() {
        for file in [".env.local", ".env"] {
            let path = dir.join(file);
            if let Ok(body) = std::fs::read_to_string(&path)
                && let Some(url) = dotenv_value(&body, "VITE_CONVEX_URL").and_then(clean_url)
            {
                log::info!("convex url from {}", path.display());
                return url;
            }
        }
    }
    DEFAULT_CONVEX_URL.to_string()
}

fn clean_url(raw: String) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Directories to probe for env files: the Buddy checkout roots found above
/// the working directory and above the executable. Only a directory that is
/// recognisably this repository counts, so a binary launched from inside an
/// unrelated project cannot pick up that project's `VITE_CONVEX_URL`.
fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push_chain = |start: Option<PathBuf>| {
        let mut current = start;
        while let Some(dir) = current {
            if is_buddy_checkout(&dir) && !roots.contains(&dir) {
                roots.push(dir.clone());
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    };
    push_chain(std::env::current_dir().ok());
    push_chain(
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
    );
    roots
}

/// The repository root: the Electron package, the Convex functions and the
/// Rust workspace side by side.
fn is_buddy_checkout(dir: &Path) -> bool {
    dir.join("package.json").is_file()
        && dir.join("convex").is_dir()
        && dir.join("rust").join("Cargo.toml").is_file()
}

/// Minimal dotenv lookup: `KEY=value`, optional `export`, `#` comments,
/// surrounding single or double quotes stripped.
pub fn dotenv_value(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim();
        // Quoted values keep everything inside the quotes (including `#`);
        // unquoted values stop at an inline ` #` comment, as dotenv does.
        let v = if let Some(inner) = v.strip_prefix('"').and_then(|s| s.split('"').next()) {
            inner
        } else if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.split('\'').next()) {
            inner
        } else {
            v.split(" #").next().unwrap_or(v).trim()
        };
        Some(v.to_string())
    })
}

#[derive(Debug, Clone)]
pub enum ConvexUpdate {
    Stats(BuddyStats),
    RepoBookmarkCount(usize),
    /// A failure attributed to one subscription (or the connection itself),
    /// so the UI can clear it only when that same source recovers.
    Error {
        source: ConvexSource,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvexSource {
    Connection,
    Stats,
    Bookmarks,
}

impl ConvexUpdate {
    /// The sources a successful update proves healthy.
    pub fn recovered_sources(&self) -> &'static [ConvexSource] {
        match self {
            ConvexUpdate::Stats(_) => &[ConvexSource::Connection, ConvexSource::Stats],
            ConvexUpdate::RepoBookmarkCount(_) => {
                &[ConvexSource::Connection, ConvexSource::Bookmarks]
            }
            ConvexUpdate::Error { .. } => &[],
        }
    }
}

/// Decides how loudly a Convex update is logged: a source's first failure at
/// WARN, repeats while it stays down at DEBUG, and its recovery once at INFO.
/// The subscriptions retry for as long as the app runs, so without this an
/// app left open without a backend logs a warning on every retry.
#[derive(Debug, Default)]
pub struct OutageLog {
    down: Vec<ConvexSource>,
}

impl OutageLog {
    pub fn failed(&mut self, source: ConvexSource) -> log::Level {
        if self.down.contains(&source) {
            log::Level::Debug
        } else {
            self.down.push(source);
            log::Level::Warn
        }
    }

    /// The sources among `sources` that were down until now.
    pub fn recovered(&mut self, sources: &[ConvexSource]) -> Vec<ConvexSource> {
        let (back, still_down) = self.down.iter().partition(|s| sources.contains(s));
        self.down = still_down;
        back
    }
}

fn result_json(result: FunctionResult) -> Result<serde_json::Value, String> {
    match result {
        FunctionResult::Value(value) => Ok(value.export()),
        FunctionResult::ErrorMessage(message) => Err(message),
        FunctionResult::ConvexError(error) => Err(error.to_string()),
    }
}

/// Runs until the receiver is dropped. Connection and subscription failures
/// (backend not started yet, closed by the server) are reported through the
/// channel and retried with backoff, so starting `npx convex dev` after the
/// app is already open still connects.
pub async fn run_dashboard_subscriptions(url: String, tx: UnboundedSender<ConvexUpdate>) {
    let mut attempt: u32 = 0;
    while !tx.is_closed() {
        match subscribe_once(&url, &tx).await {
            Ok(()) => return,
            Err((message, delivered)) => {
                // A connection that delivered data restarts the backoff, so a
                // transient drop after hours of service reconnects quickly.
                if delivered {
                    attempt = 0;
                }
                attempt += 1;
                let delay = Duration::from_secs((5u64 << attempt.min(4)).min(60));
                let _ = tx.unbounded_send(ConvexUpdate::Error {
                    source: ConvexSource::Connection,
                    message: format!("{message}; retrying in {}s", delay.as_secs()),
                });
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// One connection lifetime. `Ok` only when the receiver went away; `Err`
/// carries the message and whether any update was delivered first.
async fn subscribe_once(
    url: &str,
    tx: &UnboundedSender<ConvexUpdate>,
) -> Result<(), (String, bool)> {
    let hint = "is the local backend running?";
    crate::http::initialize_tls();
    let mut client = ConvexClient::new(url).await.map_err(|err| {
        (
            format!("Convex connection failed: {err} ({url}); {hint}"),
            false,
        )
    })?;
    let stats = client
        .subscribe("buddyStats:get", BTreeMap::new())
        .await
        .map_err(|err| {
            (
                format!("buddyStats:get failed: {err} ({url}); {hint}"),
                false,
            )
        })?;
    let bookmarks = client
        .subscribe("repoBookmarks:list", BTreeMap::new())
        .await
        .map_err(|err| {
            (
                format!("repoBookmarks:list failed: {err} ({url}); {hint}"),
                false,
            )
        })?;

    let mut merged = futures::stream::select(
        stats.map(|result| ("stats", result)),
        bookmarks.map(|result| ("bookmarks", result)),
    );

    let mut delivered = false;
    while let Some((source, result)) = merged.next().await {
        let update = match (source, result_json(result)) {
            ("stats", Ok(json)) => ConvexUpdate::Stats(BuddyStats::from_json(&json)),
            ("bookmarks", Ok(json)) => {
                ConvexUpdate::RepoBookmarkCount(json.as_array().map(Vec::len).unwrap_or(0))
            }
            ("stats", Err(message)) => ConvexUpdate::Error {
                source: ConvexSource::Stats,
                message,
            },
            (_, Err(message)) => ConvexUpdate::Error {
                source: ConvexSource::Bookmarks,
                message,
            },
            _ => continue,
        };
        if !matches!(update, ConvexUpdate::Error { .. }) {
            delivered = true;
        }
        if tx.unbounded_send(update).is_err() {
            return Ok(());
        }
    }
    // Keep the client alive until the streams end so subscriptions stay open.
    drop(client);
    Err((
        format!(
            "Convex connection to {url} closed; start the local backend (npx convex dev) or set BUDDY_CONVEX_URL"
        ),
        delivered,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outage_warns_once_and_logs_its_recovery_once() {
        let mut log = OutageLog::default();
        let stats = ConvexUpdate::Stats(BuddyStats::from_json(&serde_json::json!({})));
        // Nothing was down, so a first success reports no recovery.
        assert!(log.recovered(stats.recovered_sources()).is_empty());
        let retries: Vec<_> = (0..5)
            .map(|_| log.failed(ConvexSource::Connection))
            .collect();
        assert_eq!(retries[0], log::Level::Warn);
        assert!(retries[1..].iter().all(|level| *level == log::Level::Debug));
        assert_eq!(
            log.recovered(stats.recovered_sources()),
            vec![ConvexSource::Connection]
        );
        assert!(log.recovered(stats.recovered_sources()).is_empty());
        // The next outage warns again.
        assert_eq!(log.failed(ConvexSource::Connection), log::Level::Warn);
    }

    #[test]
    fn one_subscription_recovering_leaves_the_other_down() {
        let mut log = OutageLog::default();
        log.failed(ConvexSource::Stats);
        log.failed(ConvexSource::Bookmarks);
        let bookmarks = ConvexUpdate::RepoBookmarkCount(2);
        assert_eq!(
            log.recovered(bookmarks.recovered_sources()),
            vec![ConvexSource::Bookmarks]
        );
        assert_eq!(log.failed(ConvexSource::Stats), log::Level::Debug);
    }

    #[test]
    fn reads_vite_convex_url_from_dotenv_text() {
        let body = "# Deployment used by `npx convex dev`\nCONVEX_DEPLOYMENT=anonymous:anonymous-hs-buddy\nVITE_CONVEX_URL=http://127.0.0.1:3210\nVITE_CONVEX_SITE_URL=http://127.0.0.1:3211\n";
        assert_eq!(
            dotenv_value(body, "VITE_CONVEX_URL").as_deref(),
            Some("http://127.0.0.1:3210")
        );
        assert_eq!(dotenv_value(body, "MISSING"), None);
        assert_eq!(
            dotenv_value(
                "export VITE_CONVEX_URL=\"https://x.convex.cloud/\"",
                "VITE_CONVEX_URL"
            )
            .as_deref(),
            Some("https://x.convex.cloud/")
        );
        assert_eq!(
            dotenv_value("VITE_CONVEX_URL='http://localhost:3210'", "VITE_CONVEX_URL").as_deref(),
            Some("http://localhost:3210")
        );
        assert_eq!(
            dotenv_value(
                "VITE_CONVEX_URL=http://127.0.0.1:3210 # local backend",
                "VITE_CONVEX_URL"
            )
            .as_deref(),
            Some("http://127.0.0.1:3210")
        );
        assert_eq!(
            dotenv_value("VITE_CONVEX_URL=\"http://h/#frag\" # c", "VITE_CONVEX_URL").as_deref(),
            Some("http://h/#frag")
        );
    }

    #[test]
    fn strips_trailing_slash_and_rejects_empty() {
        assert_eq!(
            clean_url(" http://127.0.0.1:3210/ ".into()).as_deref(),
            Some("http://127.0.0.1:3210")
        );
        assert_eq!(clean_url("   ".into()), None);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Diagnostic only: `cargo test -p buddy-core convex_live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn convex_live() {
        let _ =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug"))
                .try_init();
        let url = convex_url();
        crate::http::initialize_tls();
        println!("connecting to {url}");
        let mut client = match ConvexClient::new(&url).await {
            Ok(c) => c,
            Err(err) => {
                println!("new() failed: {err:?}");
                return;
            }
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client.query("buddyStats:get", BTreeMap::new()),
        )
        .await;
        println!("query result: {result:?}");
    }
}
