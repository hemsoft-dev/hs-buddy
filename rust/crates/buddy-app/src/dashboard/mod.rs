//! The dashboard (`WelcomePanel.tsx`): header, card grid, quick actions, footer.

mod cards;
mod header;
pub mod primitives;

use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use buddy_core::config::{GitHubConfig, WeatherLocation};
use buddy_core::convex_data::{self, ConvexSource, ConvexUpdate, OutageLog};
use buddy_core::copilot_usage::{self, CommandCenterSummary};
use buddy_core::dashboard::{CardId, DASHBOARD_CARDS, INTERVAL_OPTIONS};
use buddy_core::finance::{self, QuoteData};
use buddy_core::pollen::{self, PollenData, PollenError};
use buddy_core::stats::{BuddyStats, WorkspacePulse};
use buddy_core::weather::{self, WeatherData};
use buddy_core::{http, pollen_credentials, secrets};
use chrono::Datelike as _;
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};

use crate::app::Section;
use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::shell::activity_bar::ACTIVITY_BAR_WIDTH;
use crate::theme::BuddyPalette;
use crate::zoom::{scaled, zpx};

/// Below this window width the grid collapses to one column (`@media (max-width: 860px)`).
const SINGLE_COLUMN_MAX_WIDTH: f32 = 860.0;
const MAX_CONTENT_WIDTH: f32 = 1200.0;
const WEATHER_DEFAULT_INTERVAL_MINUTES: u32 = 30;
const FINANCE_DEFAULT_INTERVAL_MINUTES: u32 = 15;
const CONVEX_STARTUP_GRACE: Duration = Duration::from_secs(20);

pub enum DashboardEvent {
    Navigate(Section),
}

/// Card expansion saved by an earlier run (`native.expandedCards`); cards
/// without an entry start expanded.
fn initial_expansion(saved: &BTreeMap<String, bool>) -> HashMap<CardId, bool> {
    DASHBOARD_CARDS
        .into_iter()
        .filter_map(|card| saved.get(card.key()).map(|&expanded| (card, expanded)))
        .collect()
}

/// Persisted per-card interval when it is one of the offered options,
/// else the card's default (an unsupported value would be scheduled but
/// labelled "Off").
fn interval_for(intervals: &BTreeMap<String, u32>, card: CardId, default: u32) -> u32 {
    intervals
        .get(card.key())
        .copied()
        .filter(|minutes| INTERVAL_OPTIONS.iter().any(|(option, _)| option == minutes))
        .unwrap_or(default)
}

/// Per-card refresh bookkeeping (`useAutoRefresh`).
#[derive(Debug, Clone)]
pub struct RefreshState {
    pub loading: bool,
    pub interval_minutes: u32,
    /// Last successful refresh (drives the "Updated …" label).
    pub last_refreshed: Option<Instant>,
    /// Last attempt, successful or not (drives the auto-refresh schedule so a
    /// failing endpoint is retried once per interval, not every tick).
    pub last_attempt: Option<Instant>,
}

impl RefreshState {
    fn new(interval_minutes: u32) -> Self {
        Self {
            loading: false,
            interval_minutes,
            last_refreshed: None,
            last_attempt: None,
        }
    }

    fn mark_refreshed(&mut self) {
        self.loading = false;
        let now = Instant::now();
        self.last_refreshed = Some(now);
        self.last_attempt = Some(now);
    }

    fn mark_failed(&mut self) {
        self.loading = false;
        self.last_attempt = Some(Instant::now());
    }

    fn never_attempted(&self) -> bool {
        self.last_attempt.is_none() && !self.loading
    }

    fn is_due(&self) -> bool {
        if self.loading || self.interval_minutes == 0 {
            return false;
        }
        match self.last_attempt {
            Some(at) => at.elapsed() >= Duration::from_secs(u64::from(self.interval_minutes) * 60),
            None => false,
        }
    }

    pub fn last_refreshed_label(&self) -> Option<String> {
        self.last_refreshed
            .map(|at| buddy_core::format::ago(at.elapsed().as_millis() as u64))
    }

    pub fn next_refresh_label(&self) -> Option<String> {
        if self.interval_minutes == 0 {
            return None;
        }
        // The schedule runs from the last attempt, so the countdown must too;
        // "Updated" keeps using the last success.
        let at = self.last_attempt.or(self.last_refreshed)?;
        let period = Duration::from_secs(u64::from(self.interval_minutes) * 60);
        let remaining = period.saturating_sub(at.elapsed());
        Some(buddy_core::format::countdown(remaining.as_millis() as u64))
    }
}

pub struct DashboardView {
    /// The breakpoints for the current window width and zoom, set each render.
    layout: GridLayout,
    http: reqwest::Client,
    weather_search: Entity<InputState>,
    finance_add: Entity<InputState>,
    expanded: HashMap<CardId, bool>,
    /// Unsaved toggles survive subsequent global configuration notifications.
    unsaved_expansion: HashMap<CardId, bool>,
    pollen_detail_open: bool,
    session_start: Instant,
    stats: Option<BuddyStats>,
    repo_bookmark_count: usize,
    convex_connection_error: Option<String>,
    convex_stats_error: Option<String>,
    convex_bookmarks_error: Option<String>,
    convex_started: Instant,
    command_center: CommandCenterSummary,
    copilot_errors: Vec<(String, String)>,
    weather: Option<WeatherData>,
    weather_error: Option<String>,
    weather_location: WeatherLocation,
    /// Bumped whenever the location or an explicit weather request starts;
    /// responses carrying an older generation are ignored.
    weather_generation: u64,
    pollen_generation: u64,
    finance_generation: u64,
    copilot_generation: u64,
    /// UTC (year, month) the Copilot report was fetched for; a rollover refetches.
    copilot_period: Option<(i32, u32)>,
    /// Keychain writes are serialized so the latest selection always wins.
    weather_persist_in_flight: bool,
    weather_persist_dirty: bool,
    /// A legacy plaintext location is cleared from config only once the
    /// keychain has accepted a value.
    legacy_location_pending: bool,
    /// The startup keychain lookup has not completed yet; the first weather
    /// load waits for it unless the user explicitly picks a location.
    restore_pending: bool,
    /// The user chose a location this session; a late restore must not undo it.
    location_chosen_by_user: bool,
    /// Saved city that arrived while a user lookup was running; applied if
    /// that lookup fails, dropped once a location is chosen.
    restored_location: Option<WeatherLocation>,
    /// A user-started location lookup (search or IP) has not finished yet.
    lookup_in_flight: bool,
    weather_refresh: RefreshState,
    pollen: Option<PollenData>,
    pollen_error: Option<String>,
    quotes: Vec<QuoteData>,
    /// Watchlist symbols whose last completed request failed.
    failed_symbols: Vec<String>,
    /// The dashboard is the shown section; refreshes pause while it is not.
    active: bool,
    started: bool,
    finance_error: Option<String>,
    finance_refresh: RefreshState,
    /// Inputs the last loads used, so a Settings change (Reload or an
    /// external edit) refreshes only the cards whose inputs changed.
    loaded_watchlist: Vec<String>,
    loaded_github: GitHubConfig,
    loaded_pollen_key: String,
    pollen_key_generation: u64,
    pollen_force_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DashboardEvent> for DashboardView {}

impl DashboardView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, active: bool) -> Self {
        let weather_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("City, state or zip code…"));
        let finance_add = cx
            .new(|cx| InputState::new(window, cx).placeholder("Add symbol (e.g. AAPL, ETH-USD)…"));

        let subscriptions = vec![
            cx.observe_global::<Settings>(|this, cx| this.settings_changed(cx)),
            cx.subscribe_in(&weather_search, window, |this, _, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.submit_weather_search(window, cx),
                    // The Go button's enabled state depends on the text.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            }),
            cx.subscribe_in(&finance_add, window, |this, _, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.submit_finance_add(window, cx),
                    // The Add button's enabled state depends on the text.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            }),
        ];

        let config = &Settings::global(cx).config;
        let intervals = config.native.auto_refresh.clone();
        let expanded = initial_expansion(&config.native.expanded_cards);
        let mut this = Self {
            layout: GridLayout::for_viewport(1280.0),
            http: http::client(),
            weather_search,
            finance_add,
            expanded,
            unsaved_expansion: HashMap::new(),
            pollen_detail_open: false,
            session_start: Instant::now(),
            stats: None,
            repo_bookmark_count: 0,
            convex_connection_error: None,
            convex_stats_error: None,
            convex_bookmarks_error: None,
            convex_started: Instant::now(),
            command_center: CommandCenterSummary::default(),
            copilot_errors: Vec::new(),
            weather: None,
            weather_error: None,
            weather_location: weather::default_location(),
            weather_generation: 0,
            pollen_generation: 0,
            finance_generation: 0,
            copilot_generation: 0,
            copilot_period: None,
            weather_persist_in_flight: false,
            weather_persist_dirty: false,
            legacy_location_pending: false,
            restore_pending: false,
            location_chosen_by_user: false,
            restored_location: None,
            lookup_in_flight: false,
            weather_refresh: RefreshState::new(interval_for(
                &intervals,
                CardId::Weather,
                WEATHER_DEFAULT_INTERVAL_MINUTES,
            )),
            pollen: None,
            pollen_error: None,
            quotes: Vec::new(),
            failed_symbols: Vec::new(),
            active,
            started: false,
            finance_error: None,
            finance_refresh: RefreshState::new(interval_for(
                &intervals,
                CardId::Finance,
                FINANCE_DEFAULT_INTERVAL_MINUTES,
            )),
            loaded_watchlist: Vec::new(),
            loaded_github: GitHubConfig::default(),
            loaded_pollen_key: String::new(),
            pollen_key_generation: 0,
            pollen_force_pending: false,
            _subscriptions: subscriptions,
        };

        if active {
            this.start_visible_work(cx);
        }
        this.start_ticker(cx);
        this
    }

    fn start_visible_work(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        self.restore_weather_location(cx);
        if self.card_visible(CardId::Finance, cx) {
            self.load_finance(cx);
        }
        if self.card_visible(CardId::CommandCenter, cx) {
            self.load_copilot(cx);
        }
        self.start_convex(cx);
    }

    fn card_visible(&self, card: CardId, cx: &App) -> bool {
        Settings::global(cx)
            .config
            .is_dashboard_card_visible(card.key())
    }

    // ── Async plumbing ───────────────────────────────────────────────────

    /// Run `future` on the I/O runtime and apply its output on the UI thread.
    fn run<T, F>(
        &self,
        cx: &mut Context<Self>,
        future: F,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let rx = Runtime::global(cx).spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                this.update(cx, |this, cx| {
                    apply(this, value, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Settings changed (card toggle, Reload, or an edit Electron made that a
    /// save picked up): re-run only the loads whose inputs differ.
    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        let config = Settings::global(cx).config.clone();
        self.expanded = initial_expansion(&config.native.expanded_cards);
        self.unsaved_expansion.retain(|card, expanded| {
            config.native.expanded_cards.get(card.key()) != Some(expanded)
        });
        self.expanded.extend(&self.unsaved_expansion);
        // `native.autoRefresh` may have changed on disk (Reload Configuration).
        self.weather_refresh.interval_minutes = interval_for(
            &config.native.auto_refresh,
            CardId::Weather,
            WEATHER_DEFAULT_INTERVAL_MINUTES,
        );
        self.finance_refresh.interval_minutes = interval_for(
            &config.native.auto_refresh,
            CardId::Finance,
            FINANCE_DEFAULT_INTERVAL_MINUTES,
        );
        if !self.active {
            cx.notify();
            return;
        }
        let weather_visible = config.is_dashboard_card_visible(CardId::Weather.key());
        let finance_visible = config.is_dashboard_card_visible(CardId::Finance.key());
        if finance_visible
            && (self.watchlist(cx) != self.loaded_watchlist
                || self.finance_refresh.never_attempted())
        {
            self.load_finance(cx);
        }
        let command_center_visible = config.is_dashboard_card_visible(CardId::CommandCenter.key());
        if command_center_visible
            && (config.github != self.loaded_github
                || (self.copilot_period.is_none() && !self.command_center.loading))
        {
            self.load_copilot(cx);
        }
        if weather_visible && self.weather_refresh.never_attempted() {
            // Defers to the restore callback while the keychain lookup is pending.
            self.start_weather_if_visible(cx);
        } else if weather_visible {
            self.load_pollen(false, cx);
        }
        cx.notify();
    }

    fn start_ticker(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Once a second: fire due auto-refreshes and repaint countdowns/uptime.
    fn tick(&mut self, cx: &mut Context<Self>) {
        // Like the Electron router unmounting WelcomePanel: nothing to refresh
        // or repaint while another section is shown; due work runs on return.
        if !self.active {
            return;
        }
        if self.weather_refresh.is_due() && self.card_visible(CardId::Weather, cx) {
            self.refresh_weather(cx);
        }
        if self.finance_refresh.is_due() && self.card_visible(CardId::Finance, cx) {
            self.refresh_finance(cx);
        }
        // Billing periods are UTC months; crossing one invalidates the report.
        let now = chrono::Utc::now();
        let period = (now.year(), now.month());
        if matches!(self.copilot_period, Some(loaded) if loaded != period)
            && !self.command_center.loading
            && self.card_visible(CardId::CommandCenter, cx)
        {
            self.load_copilot(cx);
        }
        cx.notify();
    }

    fn start_convex(&self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<ConvexUpdate>();
        Runtime::global(cx).spawn_detached(convex_data::run_dashboard_subscriptions(
            convex_data::convex_url(),
            tx,
        ));
        cx.spawn(async move |this, cx| {
            let mut outages = OutageLog::default();
            while let Some(update) = rx.next().await {
                for source in outages.recovered(update.recovered_sources()) {
                    log::info!("convex ({source:?}): connected again");
                }
                let applied = this.update(cx, |this, cx| {
                    match update {
                        ConvexUpdate::Stats(stats) => {
                            this.stats = Some(stats);
                            this.convex_connection_error = None;
                            this.convex_stats_error = None;
                        }
                        ConvexUpdate::RepoBookmarkCount(count) => {
                            this.repo_bookmark_count = count;
                            this.convex_connection_error = None;
                            this.convex_bookmarks_error = None;
                        }
                        ConvexUpdate::Error { source, message } => {
                            log::log!(outages.failed(source), "convex ({source:?}): {message}");
                            let slot = match source {
                                ConvexSource::Connection => &mut this.convex_connection_error,
                                ConvexSource::Stats => &mut this.convex_stats_error,
                                ConvexSource::Bookmarks => &mut this.convex_bookmarks_error,
                            };
                            *slot = Some(message);
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // ── Loads ────────────────────────────────────────────────────────────

    fn restore_weather_location(&mut self, cx: &mut Context<Self>) {
        // An older Electron build may have left a plaintext `ui.weatherLocation`.
        // It is shown until the keychain answers and loses to a keychain value,
        // which is always the newer of the two: the app writes the keychain
        // only after the user chose a city or migrated this very value.
        if let Some(legacy) = Settings::global(cx).config.ui.weather_location.clone() {
            self.weather_location = legacy;
            // The plaintext copy is removed in the persist callback, only after
            // the keychain has accepted a value; a failed write keeps it.
            self.legacy_location_pending = true;
        }
        let visible = self.card_visible(CardId::Weather, cx);
        self.weather_refresh.loading = visible;
        self.restore_pending = true;
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(secrets::try_load_weather_location)
                    .await
                    .unwrap_or_else(|err| Err(err.to_string()))
            },
            |this, saved, cx| {
                this.restore_pending = false;
                // An unreadable keychain is not an empty one: it may hold a
                // newer city, so the legacy plaintext must not be written
                // over it; it stays in the config for a later attempt.
                let (saved, keychain_readable) = match saved {
                    Ok(saved) => (saved, true),
                    Err(err) => {
                        log::warn!("could not read the saved weather location: {err}");
                        (None, false)
                    }
                };
                // A lookup the user started meanwhile wins; its loads are
                // already running. Keep the saved city so a failed lookup can
                // still fall back to it instead of the default.
                if this.location_chosen_by_user {
                    // Only a lookup still running can fail back to the saved
                    // city; a choice already applied stands.
                    if this.lookup_in_flight {
                        this.restored_location = saved;
                    }
                    return;
                }
                if let Some(location) = saved {
                    this.weather_location = location;
                }
                if this.legacy_location_pending && keychain_readable {
                    // Moves the city to the keychain (a re-write of the same
                    // value when it came from there) and clears the plaintext
                    // on success.
                    this.persist_weather_location(cx);
                }
                this.weather_refresh.loading = false;
                this.start_weather_if_visible(cx);
            },
        );
    }

    fn start_weather_if_visible(&mut self, cx: &mut Context<Self>) {
        if self.card_visible(CardId::Weather, cx) && !self.restore_pending {
            self.load_weather(cx);
            self.load_pollen(true, cx);
        }
    }

    /// Write the current location to the keychain, one write at a time; a
    /// selection made while a write is running is persisted right after it.
    fn persist_weather_location(&mut self, cx: &mut Context<Self>) {
        if self.weather_persist_in_flight {
            self.weather_persist_dirty = true;
            return;
        }
        self.weather_persist_in_flight = true;
        self.weather_persist_dirty = false;
        let location = self.weather_location.clone();
        self.run(
            cx,
            async move {
                let saved = location.clone();
                let result =
                    tokio::task::spawn_blocking(move || secrets::save_weather_location(&location))
                        .await;
                (saved, result)
            },
            |this, (saved, result), cx| {
                this.weather_persist_in_flight = false;
                match result {
                    Ok(Ok(())) => {
                        // The plaintext copy goes only once the keychain holds
                        // the value, and the flag stays set until that removal
                        // is on disk so a failed write is retried next time.
                        if this.legacy_location_pending
                            && Settings::update(cx, |config| config.ui.weather_location = None)
                        {
                            this.legacy_location_pending = false;
                        }
                    }
                    Ok(Err(err)) => log::warn!("could not remember weather location: {err}"),
                    Err(err) => log::warn!("keychain task failed: {err}"),
                }
                if this.weather_persist_dirty || saved != this.weather_location {
                    this.persist_weather_location(cx);
                }
            },
        );
    }

    /// Start a new weather request generation; older in-flight replies are dropped.
    fn next_weather_generation(&mut self) -> u64 {
        self.weather_generation += 1;
        self.weather_generation
    }

    fn load_weather(&mut self, cx: &mut Context<Self>) {
        self.weather_refresh.loading = true;
        let http = self.http.clone();
        let location = self.weather_location.clone();
        let generation = self.next_weather_generation();
        self.run(
            cx,
            async move { weather::fetch_weather(&http, &location).await },
            move |this, result, _| {
                if generation != this.weather_generation {
                    return;
                }
                match result {
                    Ok(data) => {
                        this.weather = Some(data);
                        this.weather_error = None;
                        this.weather_refresh.mark_refreshed();
                    }
                    Err(err) => {
                        // Keep the last good forecast on screen, flag it stale,
                        // and retry on the normal schedule.
                        this.weather_error = Some(err);
                        this.weather_refresh.mark_failed();
                    }
                }
            },
        );
    }

    fn load_pollen(&mut self, force: bool, cx: &mut Context<Self>) {
        let profile = buddy_core::config::config_path();
        self.pollen_force_pending |= force;
        self.pollen_key_generation += 1;
        let generation = self.pollen_key_generation;
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    let path = profile.map_err(|_| pollen_credentials::CredentialError::Profile)?;
                    pollen_credentials::load_at(&path)
                })
                .await
                .unwrap_or(Err(pollen_credentials::CredentialError::Unavailable))
            },
            move |this, result, cx| {
                if generation != this.pollen_key_generation {
                    return;
                }
                let key = match result {
                    Ok(key) => key.unwrap_or_default(),
                    Err(error) => {
                        this.pollen_generation += 1;
                        this.pollen_error = Some(error.to_string());
                        return;
                    }
                };
                let force = std::mem::take(&mut this.pollen_force_pending);
                if force || key != this.loaded_pollen_key {
                    this.fetch_pollen_with_key(key, cx);
                }
            },
        );
    }

    fn fetch_pollen_with_key(&mut self, key: String, cx: &mut Context<Self>) {
        let http = self.http.clone();
        let (lat, lon) = (
            self.weather_location.latitude,
            self.weather_location.longitude,
        );
        self.loaded_pollen_key = key.clone();
        self.pollen_generation += 1;
        let generation = self.pollen_generation;
        self.run(
            cx,
            async move { pollen::fetch_pollen(&http, lat, lon, &key).await },
            move |this, result, _| {
                if generation != this.pollen_generation {
                    return;
                }
                match result {
                    Ok(data) => {
                        this.pollen = Some(data);
                        this.pollen_error = None;
                    }
                    Err(PollenError::NoApiKey) => {
                        this.pollen = None;
                        this.pollen_error = None;
                    }
                    Err(PollenError::Message(message)) => this.pollen_error = Some(message),
                }
            },
        );
    }

    fn load_finance(&mut self, cx: &mut Context<Self>) {
        // Normalized and deduplicated: one request and one row per symbol.
        let watchlist = self.watchlist(cx);
        self.loaded_watchlist = watchlist.clone();
        if watchlist.is_empty() {
            // Invalidate any in-flight request so its late failure cannot
            // surface on an empty card, and drop the previous list's error.
            self.finance_generation += 1;
            self.quotes.clear();
            self.failed_symbols.clear();
            self.finance_error = None;
            self.finance_refresh.mark_refreshed();
            return;
        }
        self.finance_refresh.loading = true;
        let http = self.http.clone();
        self.finance_generation += 1;
        let generation = self.finance_generation;
        self.run(
            cx,
            async move { finance::fetch_quote_batch(&http, &watchlist).await },
            move |this, batch, cx| {
                if generation != this.finance_generation {
                    return;
                }
                // Reconcile against the live watchlist: a symbol removed while
                // this request was in flight must not come back, and quotes
                // kept from before must not outlive their symbols.
                let current = this.watchlist(cx);
                this.quotes.retain(|q| current.contains(&q.symbol));
                this.failed_symbols.retain(|s| current.contains(s));
                let all_failed = batch.quotes.is_empty() && !batch.failed.is_empty();
                let first_error = batch.failed.first().map(|(_, err)| err.clone());
                if all_failed && !this.quotes.is_empty() {
                    // An outage is not a verdict on any symbol: keep the last
                    // quotes on screen. Every symbol without one did fail just
                    // now, so list those (a newly added ticker included) with
                    // their remove buttons.
                    this.failed_symbols = batch
                        .failed
                        .into_iter()
                        .map(|(symbol, _)| symbol)
                        .filter(|symbol| {
                            current.contains(symbol)
                                && !this.quotes.iter().any(|q| &q.symbol == symbol)
                        })
                        .collect();
                    this.finance_error = first_error;
                    this.finance_refresh.mark_failed();
                    return;
                }
                let mut quotes = batch.quotes;
                quotes.retain(|q| current.contains(&q.symbol));
                this.quotes = quotes;
                this.failed_symbols = batch
                    .failed
                    .into_iter()
                    .map(|(symbol, _)| symbol)
                    .filter(|symbol| current.contains(symbol))
                    .collect();
                if all_failed {
                    this.finance_error = first_error;
                    this.finance_refresh.mark_failed();
                } else {
                    this.finance_error = None;
                    this.finance_refresh.mark_refreshed();
                }
            },
        );
    }

    fn load_copilot(&mut self, cx: &mut Context<Self>) {
        let github = Settings::global(cx).config.github.clone();
        self.loaded_github = github.clone();
        self.command_center.loading = true;
        self.copilot_generation += 1;
        let generation = self.copilot_generation;
        // The report is for the month the request was made in, even if it
        // completes after a rollover; the ticker then refetches.
        let requested_at = chrono::Utc::now();
        let period = (requested_at.year(), requested_at.month());
        self.run(
            cx,
            async move { copilot_usage::fetch_report(&github, requested_at).await },
            move |this, report, _| {
                if generation != this.copilot_generation {
                    return;
                }
                this.copilot_period = Some(period);
                for (username, error) in &report.errors {
                    log::warn!("copilot usage for {username}: {error}");
                }
                this.command_center = report.summary;
                this.copilot_errors = report.errors;
            },
        );
    }

    fn set_weather_location(&mut self, location: WeatherLocation, cx: &mut Context<Self>) {
        self.location_chosen_by_user = true;
        self.lookup_in_flight = false;
        self.restored_location = None;
        self.weather_location = location;
        self.weather = None;
        self.pollen = None;
        self.persist_weather_location(cx);
        self.load_weather(cx);
        self.load_pollen(true, cx);
    }

    // ── State accessors used by the cards ───────────────────────────────

    pub fn is_expanded(&self, card: CardId) -> bool {
        self.expanded.get(&card).copied().unwrap_or(true)
    }

    /// Flip a card open or closed and remember it for the next launch. A
    /// failed save (logged) still flips the card for this session.
    pub fn toggle_expanded(&mut self, card: CardId, cx: &mut Context<Self>) {
        self.toggle_expanded_with(card, cx, |cx, card, next| {
            Settings::update(cx, |config| {
                config.set_dashboard_card_expanded(card.key(), next)
            })
        });
    }

    fn toggle_expanded_with(
        &mut self,
        card: CardId,
        cx: &mut Context<Self>,
        save: impl FnOnce(&mut gpui_kit::App, CardId, bool) -> bool,
    ) {
        let next = !self.is_expanded(card);
        self.expanded.insert(card, next);
        let saved = save(cx, card, next);
        if saved {
            self.unsaved_expansion.remove(&card);
        } else {
            self.unsaved_expansion.insert(card, next);
        }
        cx.notify();
    }

    /// `useLiveUptime`: stored uptime plus the running session.
    pub fn live_uptime_ms(&self) -> u64 {
        let stats = self.stats.as_ref();
        let stored = stats.map(|s| s.total_uptime_ms).unwrap_or(0);
        let session = match stats.and_then(|s| s.last_session_start) {
            Some(started_ms) => {
                let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
                now_ms.saturating_sub(started_ms)
            }
            None => self.session_start.elapsed().as_millis() as u64,
        };
        stored + session
    }

    pub fn command_center(&self) -> &CommandCenterSummary {
        &self.command_center
    }

    pub fn copilot_error(&self) -> Option<String> {
        self.copilot_errors
            .first()
            .map(|(username, error)| format!("{username}: {error}"))
    }

    /// Whether the dashboard uses its narrow (`max-width: 680px`) layout.
    pub fn narrow(&self) -> bool {
        self.layout.narrow
    }

    /// See [`GridLayout::stat_columns`].
    pub fn stat_columns(&self, card: CardId, min_tile: f32) -> usize {
        self.layout.stat_columns(card, min_tile)
    }

    pub fn pulse(&self) -> WorkspacePulse {
        let stats = self.stats.clone().unwrap_or_default();
        // Active PR counts need the pull-request views, which are not ported yet.
        WorkspacePulse::from_stats(&stats, None, self.repo_bookmark_count as u64)
    }

    /// Connection error, or a reachability hint once the first load is overdue.
    pub fn convex_error(&self) -> Option<String> {
        if let Some(error) = self
            .convex_connection_error
            .as_ref()
            .or(self.convex_stats_error.as_ref())
            .or(self.convex_bookmarks_error.as_ref())
        {
            return Some(error.clone());
        }
        (self.stats.is_none() && self.convex_started.elapsed() > CONVEX_STARTUP_GRACE)
            .then(|| format!("Convex not reachable yet ({})", convex_data::convex_url()))
    }

    pub fn stats_loaded(&self) -> bool {
        self.stats.is_some()
    }

    pub fn weather(&self) -> Option<&WeatherData> {
        self.weather.as_ref()
    }

    pub fn weather_error(&self) -> Option<&str> {
        self.weather_error.as_deref()
    }

    pub fn weather_caption(&self) -> String {
        self.weather
            .as_ref()
            .map(|w| w.location_name.clone())
            .unwrap_or_else(|| self.weather_location.name.clone())
    }

    pub fn weather_refresh(&self) -> &RefreshState {
        &self.weather_refresh
    }

    pub fn weather_search_input(&self) -> &Entity<InputState> {
        &self.weather_search
    }

    pub fn pollen(&self) -> Option<&PollenData> {
        self.pollen.as_ref()
    }

    pub fn pollen_error(&self) -> Option<&str> {
        self.pollen_error.as_deref()
    }

    pub fn pollen_detail_open(&self) -> bool {
        self.pollen_detail_open
    }

    pub fn toggle_pollen_detail(&mut self, cx: &mut Context<Self>) {
        self.pollen_detail_open = !self.pollen_detail_open;
        cx.notify();
    }

    pub fn quotes(&self) -> &[QuoteData] {
        &self.quotes
    }

    pub fn finance_error(&self) -> Option<&str> {
        self.finance_error.as_deref()
    }

    pub fn finance_refresh(&self) -> &RefreshState {
        &self.finance_refresh
    }

    pub fn finance_add_input(&self) -> &Entity<InputState> {
        &self.finance_add
    }

    /// Whether the dashboard is the shown section. Becoming active runs any
    /// refresh that fell due meanwhile.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.start_visible_work(cx);
            self.settings_changed(cx);
            self.tick(cx);
            cx.notify();
        }
    }

    /// The configured watchlist, normalized like the quotes (`AAPL` for
    /// ` aapl `) and deduplicated, in display order (`sanitizeWatchlist`).
    pub fn watchlist(&self, cx: &App) -> Vec<String> {
        let mut symbols: Vec<String> = Vec::new();
        for symbol in &Settings::global(cx).config.finance.watchlist {
            let symbol = finance::normalize_symbol(symbol);
            // Blank entries from a hand-edited file are dropped, as Electron does.
            if !symbol.is_empty() && !symbols.contains(&symbol) {
                symbols.push(symbol);
            }
        }
        symbols
    }

    /// Symbols whose last completed request failed.
    pub fn failed_symbols(&self) -> &[String] {
        &self.failed_symbols
    }

    pub fn watchlist_len(&self, cx: &App) -> usize {
        self.watchlist(cx).len()
    }

    // ── Actions ──────────────────────────────────────────────────────────

    /// Drop only this native view's cached provider results. Generation bumps
    /// prevent already-running requests from putting stale data back.
    pub fn clear_cached_data(&mut self, cx: &mut Context<Self>) {
        self.weather_generation = self.weather_generation.wrapping_add(1);
        self.pollen_generation = self.pollen_generation.wrapping_add(1);
        self.pollen_key_generation = self.pollen_key_generation.wrapping_add(1);
        self.finance_generation = self.finance_generation.wrapping_add(1);
        self.copilot_generation = self.copilot_generation.wrapping_add(1);
        self.weather = None;
        self.weather_error = None;
        self.pollen = None;
        self.pollen_error = None;
        self.loaded_pollen_key.clear();
        self.pollen_force_pending = false;
        self.quotes.clear();
        self.failed_symbols.clear();
        self.finance_error = None;
        self.command_center = CommandCenterSummary::default();
        self.copilot_errors.clear();
        self.copilot_period = None;
        self.weather_refresh = RefreshState::new(self.weather_refresh.interval_minutes);
        self.finance_refresh = RefreshState::new(self.finance_refresh.interval_minutes);
        cx.notify();
    }

    pub fn refresh_copilot_usage(&mut self, cx: &mut Context<Self>) {
        if !self.command_center.loading {
            self.load_copilot(cx);
            cx.notify();
        }
    }

    pub fn refresh_weather(&mut self, cx: &mut Context<Self>) {
        if self.weather_refresh.loading {
            return;
        }
        self.load_weather(cx);
        self.load_pollen(true, cx);
        cx.notify();
    }

    pub fn set_weather_interval(&mut self, minutes: u32, cx: &mut Context<Self>) {
        // Applied only once saved; the dropdown must not show an interval a
        // restart would undo (the observer also re-derives it from disk).
        if Settings::update(cx, |config| {
            config
                .native
                .auto_refresh
                .insert(CardId::Weather.key().to_string(), minutes);
        }) {
            self.weather_refresh.interval_minutes = minutes;
        }
        cx.notify();
    }

    /// "Use My Location": approximate from the public IP, then name it.
    pub fn use_my_location(&mut self, cx: &mut Context<Self>) {
        // Marked at the start so a pending startup restore cannot win the race.
        self.location_chosen_by_user = true;
        self.lookup_in_flight = true;
        self.weather_refresh.loading = true;
        self.weather_error = None;
        let http = self.http.clone();
        let generation = self.next_weather_generation();
        self.run(
            cx,
            async move {
                let mut location = weather::locate_by_ip(&http).await?;
                if let Some(name) =
                    weather::reverse_geocode(&http, location.latitude, location.longitude).await
                {
                    location.name = name;
                }
                Ok::<_, String>(location)
            },
            move |this, result, cx| {
                if generation != this.weather_generation {
                    return;
                }
                match result {
                    Ok(location) => this.set_weather_location(location, cx),
                    Err(err) => this.weather_lookup_failed(err, cx),
                }
            },
        );
        cx.notify();
    }

    /// A user-started location lookup failed: nothing was chosen after all.
    /// A saved city that arrived during the lookup is applied now; a restore
    /// still in flight applies on arrival as usual.
    fn weather_lookup_failed(&mut self, err: String, cx: &mut Context<Self>) {
        self.location_chosen_by_user = false;
        self.lookup_in_flight = false;
        self.weather_refresh.loading = false;
        let restored = self.restored_location.take();
        if let Some(location) = restored.clone() {
            self.weather_location = location;
        }
        // Also restart when the lookup invalidated the current location's
        // own request and nothing is on screen yet (the saved city's first
        // forecast, for instance).
        if restored.is_some() || self.weather.is_none() {
            self.start_weather_if_visible(cx);
        }
        self.weather_error = Some(err);
    }

    pub fn submit_weather_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.weather_search.read(cx).value().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.weather_search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.location_chosen_by_user = true;
        self.lookup_in_flight = true;
        self.weather_refresh.loading = true;
        self.weather_error = None;
        let http = self.http.clone();
        let generation = self.next_weather_generation();
        self.run(
            cx,
            async move { weather::search_location(&http, &query).await },
            move |this, result, cx| {
                if generation != this.weather_generation {
                    return;
                }
                match result {
                    Ok(location) => this.set_weather_location(location, cx),
                    Err(err) => this.weather_lookup_failed(err, cx),
                }
            },
        );
        cx.notify();
    }

    pub fn refresh_finance(&mut self, cx: &mut Context<Self>) {
        if self.finance_refresh.loading {
            return;
        }
        self.load_finance(cx);
        cx.notify();
    }

    pub fn set_finance_interval(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if Settings::update(cx, |config| {
            config
                .native
                .auto_refresh
                .insert(CardId::Finance.key().to_string(), minutes);
        }) {
            self.finance_refresh.interval_minutes = minutes;
        }
        cx.notify();
    }

    pub fn submit_finance_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.finance_add.read(cx).value().to_string();
        let symbol = finance::normalize_symbol(&raw);
        if !finance::is_valid_symbol(&symbol) {
            return;
        }
        // Compared normalized, so a raw ` aapl ` entry already counts as AAPL.
        let already_tracked = self.watchlist(cx).contains(&symbol);
        // Persisting the watchlist fires the Settings observer, which reloads
        // all quotes; a separate one-off request would only race with it.
        let saved = already_tracked
            || Settings::update(cx, |config| config.finance.watchlist.push(symbol.clone()));
        // The field is cleared only when the entry is tracked; a failed save
        // leaves it in place for another try.
        if saved {
            self.finance_add
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    pub fn remove_symbol(&mut self, symbol: &str, cx: &mut Context<Self>) {
        let symbol = finance::normalize_symbol(symbol);
        let saved = Settings::update(cx, |config| {
            config
                .finance
                .watchlist
                .retain(|s| finance::normalize_symbol(s) != symbol)
        });
        // The row goes only once the removal is on disk; otherwise the symbol
        // is still tracked and keeps its quote (and remove button).
        if saved {
            self.quotes.retain(|q| q.symbol != symbol);
            self.failed_symbols.retain(|s| *s != symbol);
        }
        cx.notify();
    }

    pub fn toggle_card(&mut self, card: CardId, cx: &mut Context<Self>) {
        let visible = Settings::global(cx)
            .config
            .is_dashboard_card_visible(card.key());
        Settings::update(cx, |config| {
            config.set_dashboard_card_visible(card.key(), !visible)
        });
        cx.notify();
    }

    pub fn navigate(&mut self, section: Section, cx: &mut Context<Self>) {
        cx.emit(DashboardEvent::Navigate(section));
    }

    // ── Layout ───────────────────────────────────────────────────────────

    fn visible_cards(&self, cx: &App) -> Vec<CardId> {
        let config = &Settings::global(cx).config;
        DASHBOARD_CARDS
            .into_iter()
            .filter(|card| config.is_dashboard_card_visible(card.key()))
            .collect()
    }

    fn render_card(&self, card: CardId, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match card {
            CardId::CommandCenter => cards::command_center::render(self, cx),
            CardId::WorkspacePulse => cards::workspace_pulse::render(self, cx),
            CardId::Weather => cards::weather::render(self, window, cx),
            CardId::Finance => cards::finance::render(self, window, cx),
        }
    }

    /// Two-column CSS grid semantics: span-2 cards take a row, span-1 cards pair up.
    fn grid_rows(cards: &[CardId], two_columns: bool) -> Vec<Vec<CardId>> {
        let mut rows: Vec<Vec<CardId>> = Vec::new();
        let mut pending: Option<CardId> = None;
        for &card in cards {
            if card.span() == 2 || !two_columns {
                if let Some(p) = pending.take() {
                    rows.push(vec![p]);
                }
                rows.push(vec![card]);
            } else if let Some(p) = pending.take() {
                rows.push(vec![p, card]);
            } else {
                pending = Some(card);
            }
        }
        if let Some(p) = pending {
            rows.push(vec![p]);
        }
        rows
    }

    fn render_grid(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let two_columns = self.layout.two_columns;
        let cards = self.visible_cards(cx);
        let rows = Self::grid_rows(&cards, two_columns);

        v_flex()
            .w_full()
            .gap(zpx(16.0))
            .children(rows.into_iter().map(|row| {
                let needs_spacer = two_columns && row.len() == 1 && row[0].span() == 1;
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(zpx(16.0))
                    .children(row.into_iter().map(|card| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_card(card, window, cx))
                    }))
                    .when(needs_spacer, |this| this.child(div().flex_1()))
            }))
    }

    fn render_quick_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let action = |id: &'static str, icon: IconName, label: &'static str, section: Section| {
            primitives::Pill::new(id, label)
                .icon(icon)
                .accent_icon()
                .build(
                    cx.listener(move |this, _, _, cx| this.navigate(section, cx)),
                    cx,
                )
        };
        h_flex()
            .gap(zpx(8.0))
            .flex_wrap()
            .justify_center()
            .child(action(
                "qa-my-prs",
                IconName::GitPullRequest,
                "My PRs",
                Section::GitHub,
            ))
            .child(action(
                "qa-orgs",
                IconName::Building2,
                "Organizations",
                Section::GitHub,
            ))
            .child(action(
                "qa-jobs",
                IconName::Zap,
                "Jobs",
                Section::Automation,
            ))
            .child(action(
                "qa-settings",
                IconName::Settings,
                "Settings",
                Section::Settings,
            ))
    }

    fn render_footer(&self, cx: &App) -> impl IntoElement + use<> {
        let palette = BuddyPalette::global(cx);
        h_flex()
            .items_center()
            .gap(zpx(5.0))
            .text_size(zpx(11.0))
            .text_color(palette.text_muted)
            .child("Made with")
            .child(
                Icon::new(IconName::Heart)
                    .size(zpx(12.0))
                    .text_color(crate::theme::hex("#e25555")),
            )
            .child("by HemSoft Developments")
    }
}

impl Render for DashboardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().background;
        // Lay out in pixels at 100% zoom, as the CSS breakpoints are.
        let viewport = f32::from(window.viewport_size().width) / f32::from(scaled(cx, 1.0));
        self.layout = GridLayout::for_viewport(viewport);
        let (pad_y, pad_x) = self.layout.padding();

        div()
            .id("dashboard-scroll")
            .size_full()
            .bg(background)
            .overflow_y_scrollbar()
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .px(zpx(pad_x))
                    .py(zpx(pad_y))
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(zpx(MAX_CONTENT_WIDTH))
                            .items_center()
                            .gap(zpx(20.0))
                            .child(header::render(self, cx))
                            .child(self.render_grid(window, cx))
                            .child(self.render_quick_actions(cx))
                            .child(self.render_footer(cx)),
                    ),
            )
    }
}

/// The dashboard's breakpoints for one window width, in pixels at 100% zoom
/// (zooming in shrinks that width, as browser zoom shrinks the CSS viewport).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GridLayout {
    /// `WelcomePanel.css` `max-width: 680px`.
    narrow: bool,
    /// Two cards to a row above `max-width: 860px`.
    two_columns: bool,
    /// The card grid's width.
    grid_width: f32,
}

impl GridLayout {
    fn for_viewport(viewport: f32) -> Self {
        let narrow = viewport <= 680.0;
        let mut layout = Self {
            narrow,
            two_columns: viewport > SINGLE_COLUMN_MAX_WIDTH,
            grid_width: 0.0,
        };
        let (_, pad_x) = layout.padding();
        layout.grid_width = (viewport - ACTIVITY_BAR_WIDTH - 2.0 * pad_x).min(MAX_CONTENT_WIDTH);
        layout
    }

    /// The page's vertical and horizontal padding.
    fn padding(&self) -> (f32, f32) {
        if self.narrow {
            (12.0, 16.0)
        } else {
            (20.0, 24.0)
        }
    }

    /// How many stat tiles of at least `min_tile` pixels fit in a row of
    /// `card`: 4 or 2 (the grids' column counts in `WelcomePanel.css`), or
    /// 1. A tile squeezed below its label's width breaks words mid-letter.
    fn stat_columns(&self, card: CardId, min_tile: f32) -> usize {
        const GRID_GAP: f32 = 16.0;
        const TILE_GAP: f32 = 10.0;
        let card_width = if self.two_columns && card.span() == 1 {
            (self.grid_width - GRID_GAP) / 2.0
        } else {
            self.grid_width
        };
        // `section` padding on both sides plus its 1px border.
        let padding = if self.narrow { 14.0 } else { 16.0 };
        let content = card_width - 2.0 * (padding + 1.0);
        let fits = |columns: f32| content >= columns * min_tile + (columns - 1.0) * TILE_GAP;
        if fits(4.0) {
            4
        } else if fits(2.0) {
            2
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default 1280px window at `percent` zoom.
    fn at(percent: f32) -> GridLayout {
        GridLayout::for_viewport(1280.0 * 100.0 / percent)
    }

    #[test]
    fn saved_card_expansion_is_restored() {
        let saved = BTreeMap::from([
            ("weather".to_string(), false),
            ("finance".to_string(), true),
            ("retired-card".to_string(), false),
        ]);
        let expanded = initial_expansion(&saved);
        assert_eq!(expanded.get(&CardId::Weather), Some(&false));
        assert_eq!(expanded.get(&CardId::Finance), Some(&true));
        assert_eq!(expanded.get(&CardId::CommandCenter), None);
        assert_eq!(expanded.len(), 2);
    }

    #[gpui_kit::test]
    fn hidden_dashboard_defers_io_and_applies_reloaded_card_state(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        let dashboard = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut config = buddy_core::config::AppConfig::default();
            config.native.expanded_cards.insert("weather".into(), false);
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            let mut dashboard = None;
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                let entity = cx.new(|cx| DashboardView::new(window, cx, false));
                dashboard = Some(entity.clone());
                entity
            })
            .expect("open dashboard test window");
            let dashboard = dashboard.unwrap();
            assert!(!dashboard.read(cx).started);
            assert!(!dashboard.read(cx).restore_pending);
            assert!(!dashboard.read(cx).is_expanded(CardId::Weather));
            dashboard
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let mut reloaded = Settings::global(cx).config.clone();
            reloaded.native.expanded_cards.remove("weather");
            reloaded
                .native
                .expanded_cards
                .insert("finance".into(), false);
            Settings::replace(cx, reloaded);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(dashboard.read(cx).is_expanded(CardId::Weather));
            assert!(!dashboard.read(cx).is_expanded(CardId::Finance));
            assert!(!dashboard.read(cx).started);
        });
    }

    #[test]
    fn breakpoints_follow_the_zoomed_width() {
        assert_eq!((at(100.0).narrow, at(100.0).two_columns), (false, true));
        assert_eq!((at(150.0).narrow, at(150.0).two_columns), (false, false));
        assert_eq!((at(200.0).narrow, at(200.0).two_columns), (true, false));
    }

    #[gpui_kit::test]
    fn cache_clear_invalidates_inflight_data_without_changing_settings(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = DashboardView::new(window, cx, false);
                    let before = Settings::global(cx).config.clone();
                    view.weather_error = Some("stale provider error".into());
                    view.loaded_pollen_key = "fixture-cached-secret".into();
                    view.copilot_period = Some((2026, 10));
                    view.clear_cached_data(cx);
                    assert_eq!(view.weather_generation, 1);
                    assert_eq!(view.pollen_generation, 1);
                    assert_eq!(view.pollen_key_generation, 1);
                    assert_eq!(view.finance_generation, 1);
                    assert_eq!(view.copilot_generation, 1);
                    assert!(view.weather_error.is_none());
                    assert!(view.loaded_pollen_key.is_empty());
                    assert!(view.copilot_period.is_none());
                    assert!(!view.started);
                    assert_eq!(Settings::global(cx).config, before);
                    view
                })
            })
            .unwrap();
        });
    }

    #[gpui_kit::test]
    fn failed_card_save_survives_the_disk_resync_notification(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let root = std::env::temp_dir().join(format!("buddy-card-failure-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.json");
        let config = buddy_core::config::AppConfig::default();
        std::fs::write(&path, "{}").unwrap();
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        // A directory at the staging-file path causes an actual private write failure.
        std::fs::create_dir(&temporary).unwrap();
        let dashboard = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            let mut entity = None;
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                let dashboard = cx.new(|cx| DashboardView::new(window, cx, false));
                entity = Some(dashboard.clone());
                dashboard
            })
            .unwrap();
            entity.unwrap()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            dashboard.update(cx, |dashboard, cx| {
                dashboard.toggle_expanded_with(CardId::Weather, cx, |cx, card, next| {
                    let saved = Settings::update_at(cx, &path, |config| {
                        config.set_dashboard_card_expanded(card.key(), next)
                    });
                    assert!(!saved);
                    saved
                });
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(!dashboard.read(cx).is_expanded(CardId::Weather));
            assert!(Settings::global(cx).config.native.expanded_cards.is_empty());
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stat_tiles_drop_columns_before_labels_break() {
        let pulse = |percent| at(percent).stat_columns(CardId::WorkspacePulse, 128.0);
        assert_eq!(pulse(100.0), 4);
        // Still two cards to a row, so four tiles would be ~110px wide.
        assert_eq!(pulse(120.0), 2);
        assert_eq!(pulse(150.0), 4);
        assert_eq!(pulse(300.0), 2);
        let usage = |percent| at(percent).stat_columns(CardId::CommandCenter, 180.0);
        assert_eq!(usage(100.0), 4);
        assert_eq!(usage(150.0), 2);
        assert_eq!(usage(300.0), 1);
    }
}
