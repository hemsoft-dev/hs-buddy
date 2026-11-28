//! The Finance quote cache across restarts (`finance:cache` in
//! `useFinance.ts`): `buddy_core::finance::cache` and the dashboard's use of it.

use std::path::Path;

use super::*;
use buddy_core::finance::cache::{self, SaveError};
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
    // A directory at the staging path makes the private write fail.
    let staging = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::create_dir(&staging).unwrap();
    assert!(matches!(
        cache::save(&path, &symbols(&["AAPL"]), &[quote("AAPL", 1.0)], now_ms()),
        Err(SaveError::Write(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), good);
    std::fs::remove_dir(&staging).unwrap();

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
    assert_eq!(std::fs::read(&path).unwrap(), good);
}

#[gpui_kit::test]
fn clearing_the_cache_removes_the_quote_file(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_file(&directory);
    seed(&path, &["AAPL"], &[200.0], now_ms());
    let dashboard = dashboard(cx, &["AAPL"], &path);
    cx.update(|cx| dashboard.update(cx, |view, cx| view.clear_cached_data(cx)));
    assert!(!path.exists());
}
