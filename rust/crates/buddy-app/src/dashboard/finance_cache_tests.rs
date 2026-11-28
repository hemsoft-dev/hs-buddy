//! The Finance quote cache across restarts (`finance:cache` in
//! `useFinance.ts`): `buddy_core::finance::cache` and the dashboard's use of it.

use std::path::Path;

use super::*;
use buddy_core::finance::cache::{self, Outcome, SaveError, Writer};
use gpui_kit::TestAppContext;

const MINUTE_MS: i64 = 60 * 1000;

fn quote(symbol: &str, price: f64) -> QuoteData {
    QuoteData {
        symbol: symbol.into(),
        name: symbol.into(),
        price,
        change: 1.25,
        change_percent: 0.5,
        previous_close: price - 1.25,
        market_open: true,
    }
}

fn symbols(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn cache_file(directory: &tempfile::TempDir) -> PathBuf {
    directory.path().join(cache::FILE_NAME)
}

fn seed(path: &Path, list: &[&str], prices: &[f64], fetched_at_ms: i64) {
    let quotes: Vec<_> = list.iter().zip(prices).map(|(s, p)| quote(s, *p)).collect();
    cache::save(path, &symbols(list), &quotes, fetched_at_ms).unwrap();
}

/// `valid` (a cache document) after `edit`.
fn edited(valid: &str, edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut body: serde_json::Value = serde_json::from_str(valid).unwrap();
    edit(&mut body);
    body.to_string()
}

#[test]
fn a_complete_set_is_read_back_only_within_fifteen_minutes() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let list = symbols(&["^GSPC", "BTC-USD"]);
    let now = now_ms();
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[6000.5, 97000.25],
        now - 3 * MINUTE_MS,
    );
    let cached = cache::load_fresh(&path, &list, now).unwrap();
    assert_eq!(cached.fetched_at_ms, now - 3 * MINUTE_MS);
    assert_eq!(
        cached.quotes,
        vec![quote("^GSPC", 6000.5), quote("BTC-USD", 97000.25)]
    );

    // `Date.now() - timestamp < CACHE_TTL_MS`.
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[1.0, 2.0],
        now - cache::TTL_MS + 1,
    );
    assert!(cache::load_fresh(&path, &list, now).is_some());
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[1.0, 2.0],
        now - cache::TTL_MS,
    );
    assert!(cache::load_fresh(&path, &list, now).is_none());
    // A stamp ahead of the clock could otherwise pass as fresh indefinitely.
    seed(&path, &["^GSPC", "BTC-USD"], &[1.0, 2.0], now + MINUTE_MS);
    assert!(cache::load_fresh(&path, &list, now).is_none());
}

#[test]
fn the_file_keeps_electrons_cache_document_and_its_watchlist() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(&path, &["AAPL"], &[200.0], 1_764_000_000_000);
    let body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "quotes": [{
                "symbol": "AAPL", "name": "AAPL", "price": 200.0, "change": 1.25,
                "changePercent": 0.5, "previousClose": 198.75, "marketOpen": true,
            }],
            "timestamp": 1_764_000_000_000_i64,
            "version": 3,
            "symbols": ["AAPL"],
        })
    );
}

#[test]
fn a_cache_for_another_watchlist_or_version_is_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let now = now_ms();
    seed(&path, &["^GSPC", "BTC-USD"], &[1.0, 2.0], now - MINUTE_MS);
    for list in [
        symbols(&["^GSPC"]),
        symbols(&["^GSPC", "BTC-USD", "AAPL"]),
        symbols(&["BTC-USD", "^GSPC"]),
        Vec::new(),
    ] {
        assert!(cache::load_fresh(&path, &list, now).is_none(), "{list:?}");
    }
    let mut body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    body["version"] = 2.into();
    std::fs::write(&path, body.to_string()).unwrap();
    assert!(cache::load_fresh(&path, &symbols(&["^GSPC", "BTC-USD"]), now).is_none());
}

#[test]
fn corrupt_unreadable_or_oversized_files_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let list = symbols(&["AAPL"]);
    let now = now_ms();
    assert!(cache::load_fresh(&path, &list, now).is_none(), "missing");
    seed(&path, &["AAPL"], &[200.0], now - MINUTE_MS);
    let valid = std::fs::read_to_string(&path).unwrap();

    let truncated = valid[..valid.len() / 2].to_string();
    let null_price = edited(&valid, |body| {
        body["quotes"][0]["price"] = serde_json::Value::Null
    });
    let quote_for_other_symbol = edited(&valid, |body| body["quotes"][0]["symbol"] = "MSFT".into());
    let no_quotes = edited(&valid, |body| body["quotes"] = serde_json::json!([]));
    let no_stamp = edited(&valid, |body| {
        body.as_object_mut().unwrap().remove("timestamp");
    });
    let oversized = format!("{valid}{}", " ".repeat(cache::MAX_BYTES as usize));
    for (label, body) in [
        ("empty", ""),
        ("garbage", "not json"),
        ("truncated", truncated.as_str()),
        ("null price", null_price.as_str()),
        ("other symbol", quote_for_other_symbol.as_str()),
        ("no quotes", no_quotes.as_str()),
        ("no timestamp", no_stamp.as_str()),
        ("oversized", oversized.as_str()),
    ] {
        std::fs::write(&path, body).unwrap();
        assert!(cache::load_fresh(&path, &list, now).is_none(), "{label}");
    }
    std::fs::write(&path, b"\xff\xfe\x00").unwrap();
    assert!(cache::load_fresh(&path, &list, now).is_none(), "not UTF-8");

    // A byte order mark from a hand edit is tolerated, as for config.json.
    std::fs::write(&path, format!("\u{feff}{valid}")).unwrap();
    assert!(cache::load_fresh(&path, &list, now).is_some());

    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(cache::load_fresh(&path, &list, now).is_none(), "directory");
}

#[test]
fn only_one_complete_set_is_ever_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let list = symbols(&["^GSPC", "BTC-USD"]);
    seed(&path, &["^GSPC", "BTC-USD"], &[1.0, 2.0], now_ms());
    let good = std::fs::read(&path).unwrap();
    let nan = QuoteData {
        price: f64::NAN,
        ..quote("BTC-USD", 2.0)
    };
    for quotes in [
        vec![quote("^GSPC", 9.0)],
        vec![quote("BTC-USD", 9.0), quote("^GSPC", 9.0)],
        vec![quote("^GSPC", 9.0), nan],
        vec![],
    ] {
        assert!(matches!(
            cache::save(&path, &list, &quotes, now_ms()),
            Err(SaveError::Incomplete)
        ));
    }
    assert!(matches!(
        cache::save(&path, &[], &[], now_ms()),
        Err(SaveError::Incomplete)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), good);
}

#[test]
fn the_write_is_atomic_and_a_failed_one_keeps_the_previous_cache() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(&path, &["AAPL"], &[200.0], now_ms());
    let names = || {
        let mut names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    // Written through a staging file that the rename consumes.
    assert_eq!(names(), vec![cache::FILE_NAME.to_string()]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let good = std::fs::read(&path).unwrap();
    // A failed rename (a non-empty directory where the cache belongs)
    // removes the staging file it wrote and nothing else.
    let other = directory.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let blocked = other.join(cache::FILE_NAME);
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), "x").unwrap();
    assert!(matches!(
        cache::save(
            &blocked,
            &symbols(&["AAPL"]),
            &[quote("AAPL", 1.0)],
            now_ms()
        ),
        Err(SaveError::Write(_))
    ));
    let mut left: Vec<_> = std::fs::read_dir(&other)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, vec![cache::FILE_NAME.to_string()]);
    assert_eq!(std::fs::read(&path).unwrap(), good);
    #[cfg(unix)]
    {
        // A failed staging write keeps the previous cache.
        use std::os::unix::fs::PermissionsExt as _;
        let locked = |mode| {
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(mode))
                .unwrap()
        };
        locked(0o500);
        // Skipped where the user ignores permissions (root).
        if std::fs::write(directory.path().join("probe"), "x").is_err() {
            assert!(matches!(
                cache::save(&path, &symbols(&["AAPL"]), &[quote("AAPL", 1.0)], now_ms()),
                Err(SaveError::Write(_))
            ));
            assert_eq!(std::fs::read(&path).unwrap(), good);
        }
        locked(0o700);
        let _ = std::fs::remove_file(directory.path().join("probe"));
    }

    cache::remove(&path).unwrap();
    assert!(!path.exists());
    cache::remove(&path).unwrap();
}

// ── Dashboard ────────────────────────────────────────────────────────────

/// An inactive dashboard (no provider I/O) tracking `watchlist`, with its
/// quote cache at `path`.
fn dashboard(cx: &mut TestAppContext, watchlist: &[&str], path: &Path) -> Entity<DashboardView> {
    let path = path.to_path_buf();
    let watchlist = symbols(watchlist);
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.finance.watchlist = watchlist;
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        let mut dashboard = None;
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let entity = cx.new(|cx| {
                let mut view = DashboardView::new(window, cx, false);
                view.quote_cache_path = Some(path);
                view
            });
            dashboard = Some(entity.clone());
            entity
        })
        .expect("open dashboard test window");
        dashboard.unwrap()
    })
}

/// Another dashboard window in the same app, sharing `path`.
fn another_dashboard(cx: &mut TestAppContext, path: &Path) -> Entity<DashboardView> {
    let path = path.to_path_buf();
    cx.update(|cx| {
        let mut dashboard = None;
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let entity = cx.new(|cx| {
                let mut view = DashboardView::new(window, cx, false);
                view.quote_cache_path = Some(path);
                view
            });
            dashboard = Some(entity.clone());
            entity
        })
        .expect("open another dashboard test window");
        dashboard.unwrap()
    })
}

fn begin(cx: &mut TestAppContext, dashboard: &Entity<DashboardView>) -> Vec<String> {
    cx.update(|cx| dashboard.update(cx, |view, cx| view.begin_finance_load(cx)))
        .expect("a watchlist to fetch")
        .1
}

fn apply(
    cx: &mut TestAppContext,
    dashboard: &Entity<DashboardView>,
    requested: &[String],
    batch: QuoteBatch,
) {
    cx.update(|cx| {
        dashboard.update(cx, |view, cx| {
            view.apply_finance_batch(requested, batch, cx)
        })
    });
}

fn shown(cx: &mut TestAppContext, dashboard: &Entity<DashboardView>) -> Vec<(String, f64)> {
    cx.update(|cx| {
        dashboard
            .read(cx)
            .quotes()
            .iter()
            .map(|q| (q.symbol.clone(), q.price))
            .collect()
    })
}

#[gpui_kit::test]
fn fresh_cached_quotes_are_shown_before_the_first_fetch_completes(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[6000.5, 97000.25],
        now_ms() - 3 * MINUTE_MS,
    );
    let dashboard = dashboard(cx, &["^GSPC", " btc-usd "], &path);
    assert!(shown(cx, &dashboard).is_empty());

    let requested = begin(cx, &dashboard);
    assert_eq!(requested, symbols(&["^GSPC", "BTC-USD"]));
    assert_eq!(
        shown(cx, &dashboard),
        vec![("^GSPC".into(), 6000.5), ("BTC-USD".into(), 97000.25)]
    );
    cx.update(|cx| {
        let refresh = dashboard.read(cx).finance_refresh();
        // The refresh still runs, as on every launch.
        assert!(refresh.loading);
        assert_eq!(refresh.last_refreshed_label().as_deref(), Some("3m ago"));
    });

    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![quote("^GSPC", 6100.0), quote("BTC-USD", 98000.0)],
            failed: vec![],
        },
    );
    assert_eq!(
        shown(cx, &dashboard),
        vec![("^GSPC".into(), 6100.0), ("BTC-USD".into(), 98000.0)]
    );
    // Applying the quotes did not wait for the disk: the write is queued on
    // the background executor and the file still holds the cached prices.
    let before = cache::load_fresh(&path, &requested, now_ms()).unwrap();
    assert_eq!(before.quotes[0].price, 6000.5);
    cx.run_until_parked();
    let saved = cache::load_fresh(&path, &requested, now_ms()).unwrap();
    assert_eq!(
        saved.quotes,
        vec![quote("^GSPC", 6100.0), quote("BTC-USD", 98000.0)]
    );
    assert!(now_ms() - saved.fetched_at_ms < MINUTE_MS);
}

#[gpui_kit::test]
fn the_cache_is_read_only_by_the_first_load(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let dashboard = dashboard(cx, &["AAPL"], &path);
    begin(cx, &dashboard);
    assert!(shown(cx, &dashboard).is_empty());
    // A cache appearing later (another instance) is not painted mid-session.
    seed(&path, &["AAPL"], &[200.0], now_ms());
    begin(cx, &dashboard);
    assert!(shown(cx, &dashboard).is_empty());
}

#[gpui_kit::test]
fn a_stale_cache_is_never_shown(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(&path, &["AAPL"], &[200.0], now_ms() - 16 * MINUTE_MS);
    let dashboard = dashboard(cx, &["AAPL"], &path);
    begin(cx, &dashboard);
    assert!(shown(cx, &dashboard).is_empty());
    cx.update(|cx| {
        assert!(
            dashboard
                .read(cx)
                .finance_refresh()
                .last_refreshed
                .is_none()
        )
    });
}

#[gpui_kit::test]
fn a_corrupt_cache_is_never_shown(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    std::fs::write(&path, "{\"quotes\":[{\"symbol\":\"AAPL\"").unwrap();
    let dashboard = dashboard(cx, &["AAPL"], &path);
    begin(cx, &dashboard);
    assert!(shown(cx, &dashboard).is_empty());
}

#[gpui_kit::test]
fn a_changed_watchlist_invalidates_the_cache(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[1.0, 2.0],
        now_ms() - MINUTE_MS,
    );
    let dashboard = dashboard(cx, &["^GSPC", "BTC-USD", "AAPL"], &path);
    begin(cx, &dashboard);
    assert!(shown(cx, &dashboard).is_empty());
}

#[gpui_kit::test]
fn failures_never_replace_a_complete_cache(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(
        &path,
        &["^GSPC", "BTC-USD"],
        &[6000.5, 97000.25],
        now_ms() - MINUTE_MS,
    );
    let good = std::fs::read(&path).unwrap();
    let dashboard = dashboard(cx, &["^GSPC", "BTC-USD"], &path);
    let requested = begin(cx, &dashboard);

    // Every symbol failed: the cached quotes stay on screen with the error.
    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![],
            failed: vec![
                ("^GSPC".into(), "HTTP 503".into()),
                ("BTC-USD".into(), "HTTP 503".into()),
            ],
        },
    );
    assert_eq!(shown(cx, &dashboard).len(), 2);
    cx.update(|cx| assert_eq!(dashboard.read(cx).finance_error(), Some("HTTP 503")));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), good);

    // One symbol failed: shown as today, but the partial set is not cached.
    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![quote("^GSPC", 6100.0)],
            failed: vec![("BTC-USD".into(), "HTTP 503".into())],
        },
    );
    assert_eq!(shown(cx, &dashboard), vec![("^GSPC".into(), 6100.0)]);
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), good);

    // A complete batch for a watchlist that changed in flight is not the
    // current set either.
    cx.update(|cx| {
        let mut config = Settings::global(cx).config.clone();
        config.finance.watchlist = symbols(&["^GSPC"]);
        Settings::replace(cx, config);
    });
    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![quote("^GSPC", 6200.0), quote("BTC-USD", 99000.0)],
            failed: vec![],
        },
    );
    assert_eq!(shown(cx, &dashboard), vec![("^GSPC".into(), 6200.0)]);
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), good);
}

#[gpui_kit::test]
fn clearing_the_cache_removes_the_quote_file(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(&path, &["AAPL"], &[200.0], now_ms());
    let dashboard = dashboard(cx, &["AAPL"], &path);
    let cleared = cx.update(|cx| dashboard.update(cx, |view, cx| view.clear_cached_data(cx)));
    assert_eq!(cleared, Ok(()));
    assert!(!path.exists());
}

#[gpui_kit::test]
fn a_cache_that_cannot_be_deleted_fails_the_clear(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    // A non-empty directory where the file belongs cannot be removed as one.
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), "x").unwrap();
    let dashboard = dashboard(cx, &["AAPL"], &path);
    let cleared = cx.update(|cx| dashboard.update(cx, |view, cx| view.clear_cached_data(cx)));
    let error = cleared.unwrap_err();
    assert!(
        error.starts_with("The saved Finance quotes could not be deleted ("),
        "{error}"
    );
    // The in-memory data is still dropped.
    assert!(shown(cx, &dashboard).is_empty());
}

#[gpui_kit::test]
fn a_clear_drops_a_save_that_has_not_run_yet(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let dashboard = dashboard(cx, &["AAPL"], &path);
    let requested = begin(cx, &dashboard);
    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![quote("AAPL", 200.0)],
            failed: vec![],
        },
    );
    let cleared = cx.update(|cx| dashboard.update(cx, |view, cx| view.clear_cached_data(cx)));
    assert_eq!(cleared, Ok(()));
    cx.run_until_parked();
    assert!(!path.exists());
}

#[gpui_kit::test]
fn a_settings_file_named_like_the_cache_survives_saves_and_clears(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join(cache::FILE_NAME);
    let settings =
        r#"{"finance":{"watchlist":["AAPL"]},"github":{"accounts":[{"username":"keep"}]}}"#;
    std::fs::write(&config, settings).unwrap();
    let path = cache::path_beside(&config).unwrap();
    assert_eq!(path, directory.path().join(cache::FALLBACK_FILE_NAME));
    let dashboard = dashboard(cx, &["AAPL"], &path);
    let requested = begin(cx, &dashboard);
    apply(
        cx,
        &dashboard,
        &requested,
        QuoteBatch {
            quotes: vec![quote("AAPL", 200.0)],
            failed: vec![],
        },
    );
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&config).unwrap(), settings);
    assert!(cache::load_fresh(&path, &requested, now_ms()).is_some());
    let cleared = cx.update(|cx| dashboard.update(cx, |view, cx| view.clear_cached_data(cx)));
    assert_eq!(cleared, Ok(()));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), settings);
    assert!(!path.exists());
}

#[test]
fn the_cache_is_never_the_settings_file() {
    let directory = tempfile::tempdir().unwrap();
    let beside = |name: &str| cache::path_beside(&directory.path().join(name));
    assert_eq!(
        beside("config.json"),
        Some(directory.path().join(cache::FILE_NAME))
    );
    for name in [cache::FILE_NAME, "Finance-Cache.JSON"] {
        assert_eq!(
            beside(name),
            Some(directory.path().join(cache::FALLBACK_FILE_NAME)),
            "{name}"
        );
    }
    #[cfg(unix)]
    {
        // The cache's name linking to the settings file is the same file.
        let config = directory.path().join("config.json");
        std::fs::write(&config, "{}").unwrap();
        let link = directory.path().join(cache::FILE_NAME);
        std::os::unix::fs::symlink(&config, &link).unwrap();
        assert_eq!(
            cache::path_beside(&config),
            Some(directory.path().join(cache::FALLBACK_FILE_NAME))
        );
        // Settings named like the fallback, with the first name linked to
        // them: no name is safe, so nothing is cached.
        let fallback = directory.path().join(cache::FALLBACK_FILE_NAME);
        std::fs::rename(&config, &fallback).unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&fallback, &link).unwrap();
        assert_eq!(cache::path_beside(&fallback), None);
    }
}

#[test]
fn an_older_write_never_replaces_a_newer_one() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let list = symbols(&["AAPL"]);
    let writer = Writer::default();
    let save = |ticket, price| writer.save(ticket, &path, &list, &[quote("AAPL", price)], now_ms());
    let (older, newer) = (writer.ticket(), writer.ticket());
    assert!(older < newer);
    // The newer save finishes first; the older one, arriving late, is dropped.
    assert_eq!(save(newer, 2.0).unwrap(), Outcome::Applied);
    assert_eq!(save(older, 1.0).unwrap(), Outcome::Superseded);
    let cached = cache::load_fresh(&path, &list, now_ms()).unwrap();
    assert_eq!(cached.quotes[0].price, 2.0);
    // A removal requested after a save that has not run yet wins.
    let (pending, clear) = (writer.ticket(), writer.ticket());
    assert_eq!(writer.remove(clear, &path).unwrap(), Outcome::Applied);
    assert_eq!(save(pending, 3.0).unwrap(), Outcome::Superseded);
    assert!(!path.exists());
    // Clones share one order.
    let later = writer.clone().ticket();
    assert_eq!(save(later, 4.0).unwrap(), Outcome::Applied);
}

#[gpui_kit::test]
fn a_clear_in_a_reopened_dashboard_drops_a_closed_ones_pending_save(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let closing = dashboard(cx, &["AAPL"], &path);
    let requested = begin(cx, &closing);
    apply(
        cx,
        &closing,
        &requested,
        QuoteBatch {
            quotes: vec![quote("AAPL", 200.0)],
            failed: vec![],
        },
    );
    // macOS reopens a new dashboard after the last window closed; the first
    // one's save has not run yet when the new one clears the cache.
    let reopened = another_dashboard(cx, &path);
    let cleared = cx.update(|cx| reopened.update(cx, |view, cx| view.clear_cached_data(cx)));
    assert_eq!(cleared, Ok(()));
    cx.run_until_parked();
    assert!(!path.exists());
}

#[test]
fn every_writer_for_a_path_shares_one_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let list = symbols(&["AAPL"]);
    let (first, second) = (Writer::shared(&path), Writer::shared(&path));
    let older = first.ticket();
    assert_eq!(second.remove_next(&path).unwrap(), Outcome::Applied);
    assert_eq!(
        first
            .save(older, &path, &list, &[quote("AAPL", 1.0)], now_ms())
            .unwrap(),
        Outcome::Superseded
    );
    assert!(!path.exists());
    // Another path keeps its own order.
    let elsewhere = directory.path().join("elsewhere.json");
    let ticket = Writer::shared(&elsewhere).ticket();
    assert_eq!(
        Writer::shared(&elsewhere)
            .save(ticket, &elsewhere, &list, &[quote("AAPL", 2.0)], now_ms())
            .unwrap(),
        Outcome::Applied
    );
}

#[test]
fn settings_named_finance_cache_keep_their_staging_file_and_their_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("finance-cache");
    let path = cache::path_beside(&config).unwrap();
    assert_eq!(path, directory.path().join(cache::FILE_NAME));
    // The settings writer's staging file, mid-save, when a quote save runs.
    let staging = config.with_extension(format!("json.{}.tmp", std::process::id()));
    let settings = r#"{"finance":{"watchlist":["AAPL"]}}"#;
    std::fs::write(&staging, settings).unwrap();
    seed(&path, &["AAPL"], &[200.0], now_ms());
    assert_eq!(std::fs::read_to_string(&staging).unwrap(), settings);
    std::fs::rename(&staging, &config).unwrap();
    assert_eq!(std::fs::read_to_string(&config).unwrap(), settings);
    assert!(cache::load_fresh(&path, &symbols(&["AAPL"]), now_ms()).is_some());

    // Concurrent settings and quote saves all succeed, each into its own file.
    let config_thread = config.clone();
    let settings_saves = std::thread::spawn(move || {
        for index in 0..40 {
            let (mut loaded, stamp) =
                buddy_core::config::AppConfig::load_with_stamp_from(&config_thread).unwrap();
            loaded.finance.watchlist = vec![format!("S{index}")];
            assert!(loaded.save_if_unchanged_at(&config_thread, &stamp).unwrap());
        }
    });
    for index in 0..40 {
        seed(&path, &["AAPL"], &[f64::from(index)], now_ms());
    }
    settings_saves.join().unwrap();
    let (saved, _) = buddy_core::config::AppConfig::load_with_stamp_from(&config).unwrap();
    assert_eq!(saved.finance.watchlist, vec!["S39".to_string()]);
    let cached = cache::load_fresh(&path, &symbols(&["AAPL"]), now_ms()).unwrap();
    assert_eq!(cached.quotes[0].price, 39.0);
}

#[test]
fn names_the_settings_writer_uses_are_never_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    let beside = |name: &str| cache::path_beside(&directory.path().join(name));
    // finance-cache.json is the staging-free name for these settings too.
    assert_eq!(
        beside("finance-cache"),
        Some(directory.path().join(cache::FILE_NAME))
    );
    // Settings named like a cache staging file are never written beside.
    assert_eq!(beside(".finance-cache-staging.1.n0.tmp"), None);
    assert_eq!(beside(".FINANCE-CACHE-STAGING.json"), None);
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_cache_name_is_ignored_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    let (done, finished) = std::sync::mpsc::channel();
    let fifo = path.clone();
    std::thread::spawn(move || {
        let _ = done.send(cache::load_fresh(&fifo, &symbols(&["AAPL"]), now_ms()));
    });
    let loaded = finished
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("reading a FIFO must not block");
    assert!(loaded.is_none());
    // A link to a valid cache is not followed either.
    let real = directory.path().join("real.json");
    seed(&real, &["AAPL"], &[200.0], now_ms());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert!(cache::load_fresh(&path, &symbols(&["AAPL"]), now_ms()).is_none());
    assert!(cache::load_fresh(&real, &symbols(&["AAPL"]), now_ms()).is_some());
}
