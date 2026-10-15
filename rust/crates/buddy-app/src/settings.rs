//! The loaded configuration as a GPUI global, shared by the shell and views.

use buddy_core::application_settings::SettingsError;
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

    /// A form edit can reject the fresh on-disk configuration before any
    /// write. Existing dashboard callers keep the infallible `update` API.
    pub fn try_update(
        cx: &mut App,
        edit: impl Fn(&mut AppConfig) -> Result<(), SettingsError>,
    ) -> Result<(), SettingsError> {
        let path =
            config_path().map_err(|_| SettingsError("The configuration path is unavailable."))?;
        Self::try_update_at(cx, &path, edit)
    }

    pub(crate) fn update_at(
        cx: &mut App,
        path: &std::path::Path,
        edit: impl Fn(&mut AppConfig),
    ) -> bool {
        Self::try_update_at(cx, path, |config| {
            edit(config);
            Ok(())
        })
        .is_ok()
    }

    fn try_update_at(
        cx: &mut App,
        path: &std::path::Path,
        edit: impl Fn(&mut AppConfig) -> Result<(), SettingsError>,
    ) -> Result<(), SettingsError> {
        const ATTEMPTS: usize = 3;
        let mut lock = acquire_lock(path);
        for attempt in 1..=ATTEMPTS {
            let (mut fresh, stamp) = match AppConfig::load_with_stamp_from(path) {
                Ok(loaded) => loaded,
                Err(err) => {
                    log::warn!("not saving: configuration could not be reloaded ({err})");
                    return Err(SettingsError(
                        "The configuration could not be read. Check the file before saving.",
                    ));
                }
            };
            if let Err(error) = edit(&mut fresh) {
                Self::resync(cx, path);
                return Err(error);
            }
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
                    return Ok(());
                }
                Ok(false) if attempt < ATTEMPTS => {
                    log::info!("configuration changed on disk while editing; applying again");
                }
                Ok(false) => {
                    log::warn!("configuration kept changing on disk; edit not saved");
                    Self::resync(cx, path);
                    return Err(SettingsError(
                        "The configuration kept changing. Reload and try again.",
                    ));
                }
                Err(err) => {
                    log::warn!("could not save configuration: {err}");
                    Self::resync(cx, path);
                    return Err(SettingsError(
                        "The configuration could not be saved. Check file permissions and try again.",
                    ));
                }
            }
        }
        // Every attempt was spent (conflicts, or ownership lost on the last
        // one): the file may have changed meanwhile, so follow the disk.
        log::warn!("configuration kept changing or the lock kept moving; edit not saved");
        Self::resync(cx, path);
        Err(SettingsError(
            "The configuration kept changing. Reload and try again.",
        ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use buddy_core::application_settings::{ColorField, LocalEdit, SyncedEdit};

    fn fixture(label: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("buddy-settings-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root.join("config.json")
    }

    #[gpui_kit::test]
    fn rejected_form_does_not_write_and_resyncs_the_fresh_snapshot(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let path = fixture("rejected");
        let body = r#"{"copilot":{"model":"concurrent-model"},"future":42}"#;
        std::fs::write(&path, body).unwrap();
        cx.update(|cx| {
            cx.set_global(Settings {
                config: AppConfig::default(),
            });
            let result = Settings::try_update_at(cx, &path, |config| {
                LocalEdit::Color(ColorField::Accent, "invalid".into()).apply(config)
            });
            assert!(result.is_err());
            assert_eq!(
                Settings::global(cx).config.copilot.model,
                "concurrent-model"
            );
            assert_eq!(Settings::global(cx).config.extra["future"], 42);
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[gpui_kit::test]
    fn stamp_retry_reapplies_only_the_changed_field_to_the_new_file(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let path = fixture("concurrent");
        std::fs::write(&path, "{}").unwrap();
        let wrote = std::cell::Cell::new(false);
        cx.update(|cx| {
            cx.set_global(Settings {
                config: AppConfig::default(),
            });
            let result = Settings::try_update_at(cx, &path, |config| {
                SyncedEdit::AutoRefresh(false).apply_mirror(config)?;
                if !wrote.replace(true) {
                    // A writer that ignores the advisory lock lands between
                    // our read and save. The existing stamp protocol retries.
                    std::fs::write(
                        &path,
                        r#"{"pr":{"recentlyMergedDays":60,"future":3},
                        "copilot":{"model":"newer-model","future":4},"future":5}"#,
                    )
                    .unwrap();
                }
                Ok(())
            });
            assert!(result.is_ok());
            assert!(!Settings::global(cx).config.pr.auto_refresh);
        });
        let saved = AppConfig::load_from(&path).unwrap();
        assert!(!saved.pr.auto_refresh);
        assert_eq!(saved.pr.recently_merged_days, 60.0);
        assert_eq!(saved.pr.extra["future"], 3);
        assert_eq!(saved.copilot.model, "newer-model");
        assert_eq!(saved.copilot.extra["future"], 4);
        assert_eq!(saved.extra["future"], 5);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
