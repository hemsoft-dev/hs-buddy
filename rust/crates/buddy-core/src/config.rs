//! Mirror of the electron-store configuration file (`src/types/config.ts`).
//!
//! Both the Electron app and the native app read and write the same
//! `config.json`, so unknown keys are preserved through `extra` maps and every
//! field has the same default as `defaultConfig` in TypeScript.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Environment variable that overrides the config file location.
pub const CONFIG_PATH_ENV: &str = "BUDDY_CONFIG_PATH";

/// Directory names Electron may use under the platform config root, in
/// lookup order: the packaged `productName`, the documented legacy name, and
/// the unpackaged `package.json` name.
const APP_DIR_CANDIDATES: [&str; 3] = ["Buddy", "hs-buddy", "@hemsoft/buddy"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageProvider {
    Copilot,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub username: String,
    pub org: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_provider: Option<UsageProvider>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GitHubConfig {
    pub accounts: Vec<GitHubAccount>,
    /// Durable per-account provider choice keyed by `org/username` (lowercase).
    pub usage_provider_overrides: BTreeMap<String, UsageProvider>,
    /// Product-seeded defaults that `usage_provider_overrides` may mirror.
    pub usage_provider_default_overrides: BTreeMap<String, UsageProvider>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl GitHubConfig {
    /// `getUsageProviderOverrideKey`: `org/username`, trimmed and lower-cased.
    pub fn override_key(account: &GitHubAccount) -> String {
        format!(
            "{}/{}",
            account.org.trim().to_lowercase(),
            account.username.trim().to_lowercase()
        )
    }

    /// The provider the Electron app would use. An explicit user override wins;
    /// a product-seeded default (mirrored in `usage_provider_default_overrides`)
    /// yields to an explicit inline provider on the account, as
    /// `reconcileUsageProviderOverrides` does.
    pub fn effective_usage_provider(&self, account: &GitHubAccount) -> Option<UsageProvider> {
        let key = Self::override_key(account);
        let override_ = self.usage_provider_overrides.get(&key).copied();
        let seeded = override_.is_some()
            && self.usage_provider_default_overrides.get(&key).copied() == override_;
        match (override_, account.usage_provider) {
            (Some(_), Some(inline)) if seeded => Some(inline),
            (Some(explicit), _) => Some(explicit),
            (None, inline) => inline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiConfig {
    pub theme: ThemeName,
    pub accent_color: String,
    pub font_color: String,
    pub bg_primary: String,
    pub bg_secondary: String,
    pub status_bar_bg: String,
    pub status_bar_fg: String,
    pub font_family: String,
    pub mono_font_family: String,
    pub zoom_level: f64,
    pub sidebar_width: f64,
    pub pane_sizes: Vec<f64>,
    pub display_id: i64,
    pub display_bounds: DisplayRect,
    pub display_work_area: DisplayRect,
    pub show_bookmarked_only: bool,
    pub assistant_open: bool,
    pub terminal_open: bool,
    pub terminal_panel_height: f64,
    pub favorite_users: Vec<String>,
    /// Dashboard card visibility keyed by card id; missing means visible.
    pub dashboard_cards: BTreeMap<String, bool>,
    /// Electron keeps this null and stores ciphertext at the top level.
    pub weather_location: Option<WeatherLocation>,
    pub pollen_api_key: String,
    pub enterprise_slug: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeName::Dark,
            accent_color: "#0e639c".into(),
            font_color: "#cccccc".into(),
            bg_primary: "#1e1e1e".into(),
            bg_secondary: "#252526".into(),
            status_bar_bg: "#181818".into(),
            status_bar_fg: "#9d9d9d".into(),
            font_family: "Inter".into(),
            mono_font_family: "Cascadia Code".into(),
            zoom_level: 100.0,
            sidebar_width: 300.0,
            pane_sizes: vec![300.0, 900.0],
            display_id: 0,
            display_bounds: DisplayRect::default(),
            display_work_area: DisplayRect::default(),
            show_bookmarked_only: false,
            assistant_open: false,
            terminal_open: false,
            terminal_panel_height: 300.0,
            favorite_users: Vec::new(),
            dashboard_cards: BTreeMap::new(),
            weather_location: None,
            pollen_api_key: String::new(),
            enterprise_slug: String::new(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrConfig {
    pub refresh_interval: f64,
    pub auto_refresh: bool,
    pub recently_merged_days: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            refresh_interval: 15.0,
            auto_refresh: true,
            recently_merged_days: 7.0,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FinanceConfig {
    /// Ticker symbols tracked on the Finance dashboard card.
    pub watchlist: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for FinanceConfig {
    fn default() -> Self {
        Self {
            watchlist: ["^GSPC", "^IXIC", "^DJI", "BTC-USD"]
                .into_iter()
                .map(String::from)
                .collect(),
            extra: BTreeMap::new(),
        }
    }
}

/// Settings only the native app uses. Electron's schema allows unknown
/// top-level keys, so this section round-trips untouched through its store.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NativeConfig {
    /// Auto-refresh interval in minutes per dashboard card id; 0 is off.
    pub auto_refresh: BTreeMap<String, u32>,
    /// Dashboard card expand/collapse state keyed by card id; missing means
    /// expanded (Electron keeps this per card in `localStorage`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub expanded_cards: BTreeMap<String, bool>,
    /// Activity-bar section id open when the app last changed section;
    /// `None` is the dashboard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_section: Option<String>,
    /// Keys a newer native build may add; preserved like every other section.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl NativeConfig {
    fn is_default(&self) -> bool {
        self.auto_refresh.is_empty()
            && self.expanded_cards.is_empty()
            && self.active_section.is_none()
            && self.extra.is_empty()
    }
}

/// The whole configuration file. Sections the native app does not model yet
/// (`copilot`, `automation`, `notifications`, ...) survive in `extra`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub github: GitHubConfig,
    pub ui: UiConfig,
    pub pr: PrConfig,
    pub finance: FinanceConfig,
    #[serde(skip_serializing_if = "NativeConfig::is_default")]
    pub native: NativeConfig,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Window geometry persisted by Electron's `electron-window-state` in
/// `window-state.json` next to `config.json`. Both apps read and write it, so
/// either one restores the window the other saved last.
///
/// `x`, `y`, `width` and `height` are the normal (restored) rectangle even
/// while the window is maximized or full screen.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowState {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub is_maximized: bool,
    pub is_full_screen: bool,
    /// Bounds of the display the window was on. `electron-window-state`
    /// resets to its default placement when the rectangle no longer fits a
    /// connected display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_bounds: Option<DisplayRect>,
    /// GPUI's stable display UUID. macOS bounds are relative to each screen,
    /// so the rectangle alone cannot identify the saved display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_display_uuid: Option<String>,
}

impl WindowState {
    /// `window-state.json` next to the resolved `config.json`.
    pub fn path() -> Result<PathBuf, ConfigError> {
        Ok(config_path()?.with_file_name("window-state.json"))
    }

    /// Load the saved geometry when it exists and describes a usable window.
    pub fn load() -> Option<Self> {
        Self::load_from(&Self::path().ok()?)
    }

    /// [`load`](Self::load) from `path`. A missing, unreadable or corrupt
    /// file yields `None` so the caller falls back to its default placement.
    pub fn load_from(path: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(path).ok()?;
        let state: Self = serde_json::from_str(strip_bom(&body)).ok()?;
        (state.width >= 200.0
            && state.height >= 200.0
            && state.x.is_finite()
            && state.y.is_finite())
        .then_some(state)
    }

    /// Write the state atomically (temp file + rename). The rectangle is
    /// rounded: `electron-window-state` ignores bounds that are not integers.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let rounded = Self {
            x: self.x.round(),
            y: self.y.round(),
            width: self.width.round(),
            height: self.height.round(),
            display_bounds: self.display_bounds.map(|display| DisplayRect {
                x: display.x.round(),
                y: display.y.round(),
                width: display.width.round(),
                height: display.height.round(),
            }),
            ..self.clone()
        };
        let body = serde_json::to_string(&rounded).expect("WindowState is always serializable");
        let write = |source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(write)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        write_private(&tmp, &body).map_err(write)?;
        replace_with(&tmp, path).map_err(write)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a configuration directory")]
    NoConfigDir,
    #[error("keychain migration could not be undone: {0}")]
    Keychain(String),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve the config file path, honoring `BUDDY_CONFIG_PATH` first, then the
/// first existing electron-store location, then the packaged default.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    if let Some(path) = std::env::var_os(CONFIG_PATH_ENV) {
        // Absolute, so a bare `config.json` keeps working after the process
        // changes directory and has a real parent to create.
        let path = PathBuf::from(path);
        return Ok(std::path::absolute(&path).unwrap_or(path));
    }
    let root = platform_config_root().ok_or(ConfigError::NoConfigDir)?;
    let candidates = APP_DIR_CANDIDATES
        .iter()
        .map(|dir| root.join(dir).join("config.json"))
        .collect::<Vec<_>>();
    Ok(candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone()))
}

/// The identity of the config file at one moment: its size and
/// modification time. Equal stamps mean nobody wrote in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    /// `None` when the file does not exist (or cannot be inspected).
    fn of(path: &std::path::Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// Create `path` holding `body`, readable only by its owner on Unix.
fn write_private(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    // A temp file left by an interrupted save of a process with the same id
    // would keep its old mode; no live process can own this name.
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()
}

/// Move `tmp` over `path`. `rename` replaces an existing destination on
/// every supported platform (Windows uses MOVEFILE_REPLACE_EXISTING). A
/// reader holding the file open without share-delete makes it fail on
/// Windows, usually briefly, so retry; if it keeps failing, `tmp` is removed
/// and the error returned rather than rewriting `path` in place, which would
/// expose a partial document to Electron's watcher.
fn replace_with(tmp: &Path, path: &Path) -> std::io::Result<()> {
    let mut attempts = 0;
    loop {
        match std::fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempts < 10 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => {
                let _ = std::fs::remove_file(tmp);
                return Err(err);
            }
        }
    }
}

fn platform_config_root() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        return Some(home.join("Library").join("Application Support"));
    }
    // An absolute XDG_CONFIG_HOME stands on its own (HOME may be unset in
    // service-style launches); the XDG spec says to ignore a relative one.
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        && xdg.is_absolute()
    {
        return Some(xdg);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".config"))
}

/// Drop a leading UTF-8 byte-order mark, which `serde_json` rejects. Some
/// editors and Windows PowerShell 5.1 write one into otherwise valid JSON.
pub fn strip_bom(body: &str) -> &str {
    body.strip_prefix('\u{feff}').unwrap_or(body)
}

impl AppConfig {
    /// Parse a config file body. Missing keys take TypeScript defaults.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(strip_bom(json))
    }

    /// Load from the resolved path; a missing file yields defaults.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_stamp().map(|(config, _)| config)
    }

    /// Load from `path`; a missing file yields defaults.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with_stamp_from(path).map(|(config, _)| config)
    }

    /// Load together with the file's identity at that moment, for
    /// [`save_if_unchanged`](Self::save_if_unchanged).
    pub fn load_with_stamp() -> Result<(Self, Option<FileStamp>), ConfigError> {
        Self::load_with_stamp_from(&config_path()?)
    }

    /// [`load_with_stamp`](Self::load_with_stamp) for an already resolved
    /// path, so a caller holding the lock for that path reads and writes
    /// the same file even if a higher-priority candidate appears meanwhile.
    pub fn load_with_stamp_from(path: &Path) -> Result<(Self, Option<FileStamp>), ConfigError> {
        let stamp = FileStamp::of(path);
        match std::fs::read_to_string(path) {
            Ok(body) => Self::from_json(&body)
                .map(|config| (config, stamp))
                .map_err(|source| ConfigError::Parse {
                    path: path.to_path_buf(),
                    source,
                }),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok((Self::default(), None))
            }
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Write the whole file atomically (temp file + rename) so electron-store's
    /// watcher never observes a partial document.
    pub fn save(&mut self) -> Result<(), ConfigError> {
        self.save_checked(&config_path()?, None).map(|_| ())
    }

    /// Save only if the file is still the one loaded with `stamp` (`None`
    /// meaning it did not exist). Returns `Ok(false)` when another writer
    /// got there first, so the caller can reload and apply its edit again
    /// instead of overwriting that writer's change with a stale snapshot.
    pub fn save_if_unchanged(&mut self, stamp: &Option<FileStamp>) -> Result<bool, ConfigError> {
        self.save_checked(&config_path()?, Some(stamp))
    }

    /// [`save_if_unchanged`](Self::save_if_unchanged) for an already
    /// resolved path (the one the caller holds the lock for).
    pub fn save_if_unchanged_at(
        &mut self,
        path: &Path,
        stamp: &Option<FileStamp>,
    ) -> Result<bool, ConfigError> {
        self.save_checked(path, Some(stamp))
    }

    fn save_checked(
        &mut self,
        path: &Path,
        expected: Option<&Option<FileStamp>>,
    ) -> Result<bool, ConfigError> {
        let path = path.to_path_buf();
        // Nothing to migrate or write for a snapshot that is already stale.
        if let Some(expected) = expected
            && FileStamp::of(&path) != *expected
        {
            return Ok(false);
        }
        let migrated = self.migrate_legacy_weather_location();
        let body = serde_json::to_string_pretty(self).expect("AppConfig is always serializable");
        let write = |source| ConfigError::Write {
            path: path.clone(),
            source,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(write)?;
        }
        // Per-process name: two instances saving at once must not share (or
        // unlink) each other's temp file.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        // The temp file is private from its first byte. It then takes the
        // existing file's permissions (a group-readable config stays so) or
        // stays 0600 for a brand-new config. If the existing file cannot be
        // inspected or the mode cannot be applied, the old file is kept.
        #[cfg(unix)]
        let permissions = match std::fs::metadata(&path) {
            Ok(meta) => Some(meta.permissions()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(write(err)),
        };
        write_private(&tmp, &body).map_err(write)?;
        #[cfg(unix)]
        if let Some(permissions) = permissions
            && let Err(err) = std::fs::set_permissions(&tmp, permissions)
        {
            let _ = std::fs::remove_file(&tmp);
            return Err(write(err));
        }
        // Last look before the swap: a file that changed since it was loaded
        // belongs to another writer whose edit this snapshot does not carry.
        if let Some(expected) = expected
            && FileStamp::of(&path) != *expected
        {
            let _ = std::fs::remove_file(&tmp);
            // The keychain write above belongs to this snapshot; undo it so
            // the retry (which reloads the newer file) migrates that one. If
            // the undo fails, the entry is re-pointed at the plaintext the
            // other writer left (so a later migration prefers nothing stale);
            // when that is impossible the save fails outright.
            if let Some(location) = migrated {
                self.ui.weather_location = Some(location);
                if let Err(err) = crate::secrets::clear_weather_location() {
                    match Self::load()
                        .ok()
                        .and_then(|current| current.ui.weather_location)
                    {
                        Some(newer) => crate::secrets::save_weather_location(&newer)
                            .map_err(ConfigError::Keychain)?,
                        None => return Err(ConfigError::Keychain(err)),
                    }
                }
            }
            return Ok(false);
        }
        replace_with(&tmp, &path).map_err(write)?;
        Ok(true)
    }

    /// Older Electron builds stored the weather location in plaintext under
    /// `ui.weatherLocation`. Like Electron, move it to protected storage and
    /// never write the plaintext back.
    ///
    /// The plaintext is cleared only once the keychain has accepted the value;
    /// if the keychain is unavailable the saved city is kept rather than lost.
    ///
    /// Returns the location when it was written to a previously empty
    /// keychain, so a caller that then abandons this snapshot can undo it.
    fn migrate_legacy_weather_location(&mut self) -> Option<WeatherLocation> {
        let location = self.ui.weather_location.as_ref()?;
        // A keychain entry is always the newer of the two: the app writes the
        // keychain only after the user chose a city or migrated this very
        // value. Never overwrite it with the plaintext, and do not migrate at
        // all while the keychain cannot be read (it might hold a newer city).
        match crate::secrets::try_load_weather_location() {
            Ok(Some(_)) => {
                log::info!("dropping the legacy weather location; the keychain already holds one");
                self.ui.weather_location = None;
                return None;
            }
            Ok(None) => {}
            Err(err) => {
                log::warn!("keeping legacy weather location in config; keychain unreadable: {err}");
                return None;
            }
        }
        match crate::secrets::save_weather_location(location) {
            Ok(()) => {
                log::info!("migrated legacy weather location to the keychain");
                self.ui.weather_location.take()
            }
            Err(err) => {
                log::warn!(
                    "keeping legacy weather location in config; keychain unavailable: {err}"
                );
                None
            }
        }
    }

    /// Dashboard card visibility: a card is visible unless explicitly `false`.
    pub fn is_dashboard_card_visible(&self, card_id: &str) -> bool {
        self.ui
            .dashboard_cards
            .get(card_id)
            .copied()
            .unwrap_or(true)
    }

    pub fn set_dashboard_card_visible(&mut self, card_id: &str, visible: bool) {
        self.ui.dashboard_cards.insert(card_id.to_string(), visible);
    }

    /// Dashboard card expansion: a card is expanded unless explicitly `false`.
    pub fn is_dashboard_card_expanded(&self, card_id: &str) -> bool {
        self.native
            .expanded_cards
            .get(card_id)
            .copied()
            .unwrap_or(true)
    }

    pub fn set_dashboard_card_expanded(&mut self, card_id: &str, expanded: bool) {
        self.native
            .expanded_cards
            .insert(card_id.to_string(), expanded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_byte_order_mark_is_ignored() {
        let config = AppConfig::from_json("\u{feff}{\"ui\":{\"theme\":\"light\"}}").unwrap();
        assert_eq!(config.ui.theme, ThemeName::Light);
    }

    #[test]
    fn a_config_file_with_a_byte_order_mark_loads_and_saves() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("buddy-bom-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, "\u{feff}{\"ui\":{\"theme\":\"light\"},\"x\":1}").unwrap();
        let (mut config, stamp) = AppConfig::load_with_stamp_from(&path).unwrap();
        config.set_dashboard_card_visible("weather", false);
        let saved = config.save_if_unchanged_at(&path, &stamp).unwrap();
        let reloaded = AppConfig::load_from(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(saved);
        assert_eq!(reloaded.ui.theme, ThemeName::Light);
        assert!(!reloaded.is_dashboard_card_visible("weather"));
        assert_eq!(reloaded.extra.get("x"), Some(&serde_json::json!(1)));
        assert!(serde_json::from_str::<serde_json::Value>(&body).is_ok());
    }

    #[test]
    fn empty_object_yields_typescript_defaults() {
        let config = AppConfig::from_json("{}").unwrap();
        assert_eq!(config, AppConfig::default());
        assert_eq!(config.ui.accent_color, "#0e639c");
        assert_eq!(config.ui.pane_sizes, vec![300.0, 900.0]);
        assert_eq!(config.finance.watchlist.len(), 4);
        assert!(config.pr.auto_refresh);
    }

    #[test]
    fn parses_electron_store_shape_and_preserves_unknown_keys() {
        let json = r#"{
          "weatherLocationCiphertext": "abc",
          "github": {
            "accounts": [{ "username": "hemsoft", "org": "hemsoft", "usageProvider": "codex" }],
            "usageProviderOverrides": { "hemsoft/hemsoft": "codex" }
          },
          "ui": {
            "theme": "light",
            "dashboardCards": { "finance": false },
            "zoomLevel": 110,
            "someFutureFlag": true
          },
          "copilot": { "model": "claude-sonnet-4.5" }
        }"#;
        let config = AppConfig::from_json(json).unwrap();
        assert_eq!(config.ui.theme, ThemeName::Light);
        assert_eq!(config.ui.zoom_level, 110.0);
        assert_eq!(
            config.github.accounts[0].usage_provider,
            Some(UsageProvider::Codex)
        );
        assert!(!config.is_dashboard_card_visible("finance"));
        assert!(config.is_dashboard_card_visible("weather"));
        assert_eq!(
            config
                .github
                .usage_provider_overrides
                .get("hemsoft/hemsoft"),
            Some(&UsageProvider::Codex)
        );
        let inline = GitHubAccount {
            username: "Franz".into(),
            org: "Relias".into(),
            repo_root: None,
            usage_provider: None,
            extra: BTreeMap::new(),
        };
        assert_eq!(GitHubConfig::override_key(&inline), "relias/franz");
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[0]),
            Some(UsageProvider::Codex)
        );
        assert_eq!(config.github.effective_usage_provider(&inline), None);

        // A seeded default yields to an explicit inline provider; a user override does not.
        let mut github = config.github.clone();
        github.accounts[0].usage_provider = Some(UsageProvider::Copilot);
        github
            .usage_provider_default_overrides
            .insert("hemsoft/hemsoft".into(), UsageProvider::Codex);
        assert_eq!(
            github.effective_usage_provider(&github.accounts[0]),
            Some(UsageProvider::Copilot)
        );
        github.usage_provider_default_overrides.clear();
        assert_eq!(
            github.effective_usage_provider(&github.accounts[0]),
            Some(UsageProvider::Codex)
        );
        assert!(config.ui.extra.contains_key("someFutureFlag"));
        assert!(config.extra.contains_key("copilot"));
        assert!(config.extra.contains_key("weatherLocationCiphertext"));

        let round_trip: Value = serde_json::to_value(&config).unwrap();
        assert_eq!(round_trip["copilot"]["model"], "claude-sonnet-4.5");
        assert_eq!(round_trip["ui"]["someFutureFlag"], true);
        assert_eq!(
            round_trip["github"]["accounts"][0]["usageProvider"],
            "codex"
        );
        assert_eq!(
            round_trip["github"]["usageProviderOverrides"]["hemsoft/hemsoft"],
            "codex"
        );
    }

    /// Serializes the tests that set `BUDDY_CONFIG_PATH`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// One failing test must not poison the lock for the others.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn native_section_round_trips_and_is_omitted_when_empty() {
        let mut config = AppConfig::default();
        assert!(!serde_json::to_string(&config).unwrap().contains("native"));
        config.native.auto_refresh.insert("weather".into(), 0);
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"native\":{\"autoRefresh\":{\"weather\":0}}"));
        let back = AppConfig::from_json(&json).unwrap();
        assert_eq!(back.native.auto_refresh.get("weather"), Some(&0));
    }

    #[test]
    fn window_state_parses_electron_window_state_file() {
        let raw = r#"{"width":2629,"height":1246,"x":445,"y":63,"displayBounds":{"x":0,"y":0,"width":3440,"height":1440},"isMaximized":false,"isFullScreen":false}"#;
        let state: WindowState = serde_json::from_str(raw).unwrap();
        assert_eq!(
            (state.x, state.y, state.width, state.height),
            (445.0, 63.0, 2629.0, 1246.0)
        );
        assert!(!state.is_maximized);
        assert_eq!(
            state.display_bounds,
            Some(DisplayRect {
                x: 0.0,
                y: 0.0,
                width: 3440.0,
                height: 1440.0
            })
        );
    }

    #[test]
    fn card_expansion_and_active_section_round_trip() {
        let mut config = AppConfig::default();
        assert!(config.is_dashboard_card_expanded("weather"));
        config.set_dashboard_card_expanded("weather", false);
        config.set_dashboard_card_expanded("finance", true);
        config.native.active_section = Some("github".into());
        let json = serde_json::to_string(&config).unwrap();
        let back = AppConfig::from_json(&json).unwrap();
        assert!(!back.is_dashboard_card_expanded("weather"));
        assert!(back.is_dashboard_card_expanded("finance"));
        assert!(back.is_dashboard_card_expanded("command-center"));
        assert_eq!(back.native.active_section.as_deref(), Some("github"));
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["native"]["expandedCards"]["weather"], false);
        assert_eq!(value["native"]["activeSection"], "github");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buddy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn window_state_round_trips_normal_maximized_and_full_screen() {
        let dir = temp_dir("window-state-round-trip");
        let path = dir.join("window-state.json");
        let display = Some(DisplayRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        });
        let states =
            [(false, false), (true, false), (false, true)].map(|(is_maximized, is_full_screen)| {
                WindowState {
                    x: 120.0,
                    y: 90.0,
                    width: 1000.0,
                    height: 700.0,
                    is_maximized,
                    is_full_screen,
                    display_bounds: display,
                    native_display_uuid: Some("saved-display-uuid".into()),
                }
            });
        let loaded: Vec<_> = states
            .iter()
            .map(|state| {
                state.save_to(&path).unwrap();
                WindowState::load_from(&path)
            })
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        for (state, loaded) in states.iter().zip(loaded) {
            assert_eq!(loaded.as_ref(), Some(state));
        }
    }

    #[test]
    fn window_state_is_written_in_the_electron_format() {
        let dir = temp_dir("window-state-format");
        let path = dir.join("window-state.json");
        let state = WindowState {
            x: -1919.6,
            y: 90.4,
            width: 1000.25,
            height: 700.0,
            is_maximized: true,
            is_full_screen: false,
            display_bounds: Some(DisplayRect {
                x: -1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            }),
            native_display_uuid: None,
        };
        state.save_to(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        let _ = std::fs::remove_dir_all(&dir);
        let value: Value = serde_json::from_str(&body).unwrap();
        // electron-window-state requires integers (`Number.isInteger`).
        assert_eq!(value["x"].as_f64(), Some(-1920.0));
        assert_eq!(value["y"].as_f64(), Some(90.0));
        assert_eq!(value["width"].as_f64(), Some(1000.0));
        assert_eq!(value["height"].as_f64(), Some(700.0));
        assert_eq!(value["isMaximized"], true);
        assert_eq!(value["isFullScreen"], false);
        assert_eq!(value["displayBounds"]["x"].as_f64(), Some(-1920.0));
        assert_eq!(value["displayBounds"]["width"].as_f64(), Some(1920.0));
        assert_eq!(leftovers, 1, "the temp file is renamed into place");
    }

    #[test]
    fn corrupt_or_missing_window_state_falls_back_and_is_rewritten() {
        let dir = temp_dir("window-state-corrupt");
        let path = dir.join("window-state.json");
        let missing = WindowState::load_from(&path);
        std::fs::write(&path, "{ not json").unwrap();
        let corrupt = WindowState::load_from(&path);
        std::fs::write(&path, r#"{"x":0,"y":0,"width":50,"height":50}"#).unwrap();
        let too_small = WindowState::load_from(&path);
        let state = WindowState {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
            ..WindowState::default()
        };
        state.save_to(&path).unwrap();
        let rewritten = WindowState::load_from(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(missing.is_none());
        assert!(corrupt.is_none());
        assert!(too_small.is_none());
        assert_eq!(rewritten, Some(state));
    }

    #[test]
    fn env_override_wins_for_config_path() {
        let _guard = env_lock();
        let expected = std::env::temp_dir().join("buddy-test-config.json");
        // SAFETY: serialized by ENV_LOCK; no other thread reads this variable concurrently.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &expected) };
        let path = config_path().unwrap();
        // A relative override is made absolute against the current directory.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, "relative-config.json") };
        let relative = config_path().unwrap();
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
        assert_eq!(path, expected);
        assert!(relative.is_absolute());
        assert!(relative.ends_with("relative-config.json"));
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_restrictive_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("buddy-mode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };
        let mut config = AppConfig::load().unwrap();
        config.set_dashboard_card_visible("weather", false);
        config.save().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_if_unchanged_yields_to_a_concurrent_writer() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("buddy-stamp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, "{}").unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };
        let (mut config, stamp) = AppConfig::load_with_stamp().unwrap();
        config.set_dashboard_card_visible("weather", false);
        // Another writer (Electron, say) lands in between.
        std::fs::write(&path, "{\"ui\":{\"theme\":\"light\"},\"x\":1}").unwrap();
        let saved = config.save_if_unchanged(&stamp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        // Unchanged file: the edit applies.
        let (mut again, stamp) = AppConfig::load_with_stamp().unwrap();
        again.set_dashboard_card_visible("weather", false);
        let saved_again = again.save_if_unchanged(&stamp).unwrap();
        let body_again = std::fs::read_to_string(&path).unwrap();
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!saved);
        assert!(body.contains("\"x\": 1") || body.contains("\"x\":1"));
        assert!(saved_again);
        assert!(body_again.contains("\"weather\": false"));
        assert!(body_again.contains("\"light\""));
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_a_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("buddy-new-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };
        let mut config = AppConfig::load().unwrap();
        config.set_dashboard_card_visible("weather", false);
        config.save().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o600);
    }

    /// Runs on every CI platform, including Windows, to prove that saving over
    /// an existing config.json replaces it.
    #[test]
    fn save_replaces_an_existing_file() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!("buddy-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            "{\"ui\":{\"theme\":\"light\"},\"copilot\":{\"model\":\"x\"}}",
        )
        .unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };

        let mut first = AppConfig::load().unwrap();
        assert_eq!(first.ui.theme, ThemeName::Light);
        first.set_dashboard_card_visible("finance", false);
        first.save().unwrap();
        let mut second = AppConfig::load().unwrap();
        second.finance.watchlist.push("AAPL".into());
        second.save().unwrap();

        let reread = AppConfig::load().unwrap();
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!reread.is_dashboard_card_visible("finance"));
        assert!(reread.finance.watchlist.contains(&"AAPL".to_string()));
        assert_eq!(reread.ui.theme, ThemeName::Light);
        assert_eq!(reread.extra["copilot"]["model"], "x");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
