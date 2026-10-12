//! The loaded configuration as a GPUI global, shared by the shell and views.

use buddy_core::config::{AppConfig, UiConfig, config_path};
use buddy_core::config_lock::{ConfigLock, DEFAULT_TIMEOUT};
use gpui_kit::{App, Global};

pub struct Settings {
    pub config: AppConfig,
}

impl Global for Settings {}

impl Settings {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Re-read the file (Electron may have written it meanwhile), apply the
    /// edit to that fresh copy, persist it, and keep the in-memory copy in
    /// step. If the file cannot be read, the edit is abandoned rather than
    /// serializing a fallback over a file that may still hold recoverable
    /// data; a failed write is logged, not fatal.
    ///
    /// Returns `true` once the edit is on disk, so callers that must not
    /// forget a pending change can keep it pending.
    ///
    /// The file is shared with Electron, which may write between the reload
    /// and the save; the save is refused when the file changed meanwhile and
    /// the edit is applied again to the newer content (a few attempts).
    pub fn update(cx: &mut App, edit: impl Fn(&mut AppConfig)) -> bool {
        // Serialize the whole read-modify-write against Electron, which takes
        // the same `config.json.lock`; the stamp check below stays as the
        // guard for a writer that does not (see electron/configLock.ts).
        // One resolved path for the lock, the reads and the write: a
        // candidate file Electron creates meanwhile must not redirect them.
        let path = match config_path() {
            Ok(path) => path,
            Err(err) => {
                log::warn!("not saving: configuration path unknown ({err})");
                return false;
            }
        };
        Self::update_at(cx, &path, edit)
    }

    pub(crate) fn update_at(
        cx: &mut App,
        path: &std::path::Path,
        edit: impl Fn(&mut AppConfig),
    ) -> bool {
        const ATTEMPTS: usize = 3;
        let mut lock = acquire_lock(path);
        for attempt in 1..=ATTEMPTS {
            let (mut fresh, stamp) = match AppConfig::load_with_stamp_from(path) {
                Ok(loaded) => loaded,
                Err(err) => {
                    log::warn!("not saving: configuration could not be reloaded ({err})");
                    return false;
                }
            };
            edit(&mut fresh);
            if lock.as_ref().is_some_and(|held| !held.is_held()) {
                // Suspended past the stale window: another writer owns the
                // lock now and may be mid-edit, so do not write under it.
                // Take the lock again (the old guard leaves theirs alone) and
                // start over from a fresh read.
                log::warn!("config lock was taken over while editing; re-acquiring");
                lock = acquire_lock(path);
                continue;
            }
            match fresh.save_if_unchanged_at(path, &stamp) {
                Ok(true) => {
                    Self::install(cx, fresh);
                    return true;
                }
                Ok(false) if attempt < ATTEMPTS => {
                    log::info!("configuration changed on disk while editing; applying again");
                }
                Ok(false) => {
                    log::warn!("configuration kept changing on disk; edit not saved");
                    Self::resync(cx, path);
                    return false;
                }
                Err(err) => {
                    log::warn!("could not save configuration: {err}");
                    Self::resync(cx, path);
                    return false;
                }
            }
        }
        // Every attempt was spent (conflicts, or ownership lost on the last
        // one): the file may have changed meanwhile, so follow the disk.
        log::warn!("configuration kept changing or the lock kept moving; edit not saved");
        Self::resync(cx, path);
        false
    }

    /// After a failed save the in-memory copy must reflect the disk, not the
    /// edit that never landed, so the UI does not show a state that a
    /// restart would undo.
    fn resync(cx: &mut App, path: &std::path::Path) {
        match AppConfig::load_from(path) {
            Ok(disk) => Self::install(cx, disk),
            Err(err) => log::warn!("configuration left as last loaded; reload failed: {err}"),
        }
    }

    /// Make `config` the in-memory configuration. The GPUI theme is derived
    /// state and follows whenever an appearance field changed (Electron may
    /// have edited the shared file meanwhile), as on Reload Configuration.
    fn install(cx: &mut App, config: AppConfig) {
        let appearance_changed = appearance_differs(&cx.global::<Self>().config.ui, &config.ui);
        cx.global_mut::<Self>().config = config;
        if appearance_changed {
            let config = cx.global::<Self>().config.clone();
            crate::theme::install(&config, cx);
            cx.refresh_windows();
        }
    }

    /// Replace the in-memory configuration (used by Reload).
    pub fn replace(cx: &mut App, config: AppConfig) {
        cx.global_mut::<Self>().config = config;
    }
}

/// The `ui` fields `theme::install` reads.
fn appearance_differs(a: &UiConfig, b: &UiConfig) -> bool {
    a.theme != b.theme
        || a.accent_color != b.accent_color
        || a.bg_primary != b.bg_primary
        || a.bg_secondary != b.bg_secondary
        || a.font_color != b.font_color
        || a.font_family != b.font_family
        || a.mono_font_family != b.mono_font_family
        || a.status_bar_bg != b.status_bar_bg
        || a.status_bar_fg != b.status_bar_fg
}

/// The shared lock for `path`, or `None` (logged) when it is busy or
/// unavailable, in which case the save proceeds unlocked.
fn acquire_lock(path: &std::path::Path) -> Option<ConfigLock> {
    match ConfigLock::acquire(path, DEFAULT_TIMEOUT) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            log::warn!("config lock busy for {DEFAULT_TIMEOUT:?}; saving unlocked");
            None
        }
        Err(err) => {
            log::warn!("config lock unavailable ({err}); saving unlocked");
            None
        }
    }
}
