//! Validated settings edits. Callers apply local edits to a freshly loaded
//! config under its existing lock. Synced edits must succeed in Convex before
//! their local read-fallback mirror is updated.

use std::path::Path;

use serde_json::{Value, json};

use crate::config::{AppConfig, ThemeName, UsageProvider};
use crate::gh::is_valid_slug;

pub const REFRESH_MINUTES: [u32; 6] = [1, 5, 10, 15, 30, 60];
pub const RECENTLY_MERGED_DAYS: [u32; 5] = [7, 14, 30, 60, 90];
pub const MAX_SOUND_BYTES: u64 = 5 * 1024 * 1024;

/// Reset only the preferences named by the native reset action. The caller
/// must clear protected credentials successfully before applying this edit.
pub fn reset_local_preferences(config: &mut AppConfig) {
    let defaults = AppConfig::default();
    let ui = &mut config.ui;
    ui.theme = defaults.ui.theme;
    ui.accent_color = defaults.ui.accent_color;
    ui.font_color = defaults.ui.font_color;
    ui.bg_primary = defaults.ui.bg_primary;
    ui.bg_secondary = defaults.ui.bg_secondary;
    ui.status_bar_bg = defaults.ui.status_bar_bg;
    ui.status_bar_fg = defaults.ui.status_bar_fg;
    ui.font_family = defaults.ui.font_family;
    ui.mono_font_family = defaults.ui.mono_font_family;
    ui.dashboard_cards = defaults.ui.dashboard_cards;
    config.notifications.play_sound_on_review_complete =
        defaults.notifications.play_sound_on_review_complete;
    config.notifications.review_complete_sound_path =
        defaults.notifications.review_complete_sound_path;
    config.github.usage_provider_overrides.clear();
    config.github.usage_provider_default_overrides.clear();
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorField {
    Accent,
    Text,
    Background,
    SecondaryBackground,
    StatusBackground,
    StatusText,
}

#[derive(Debug, Clone)]
pub enum LocalEdit {
    Theme(ThemeName),
    Color(ColorField, String),
    Font {
        monospace: bool,
        family: String,
    },
    EnterpriseSlug(String),
    DashboardCard {
        id: String,
        visible: bool,
    },
    NotificationEnabled(bool),
    NotificationSound(String),
    Provider {
        account_key: String,
        provider: UsageProvider,
    },
}

impl LocalEdit {
    /// Validation precedes mutation, including account checks against this
    /// fresh configuration rather than the list the form originally showed.
    pub fn apply(&self, config: &mut AppConfig) -> Result<(), SettingsError> {
        match self {
            Self::Theme(theme) => {
                config.ui.theme = *theme;
                let colors = match theme {
                    ThemeName::Dark => [
                        "#0e639c", "#cccccc", "#1e1e1e", "#252526", "#181818", "#9d9d9d",
                    ],
                    ThemeName::Light => [
                        "#0078d4", "#1f1f1f", "#ffffff", "#f3f3f3", "#f3f3f3", "#616161",
                    ],
                };
                config.ui.accent_color = colors[0].into();
                config.ui.font_color = colors[1].into();
                config.ui.bg_primary = colors[2].into();
                config.ui.bg_secondary = colors[3].into();
                config.ui.status_bar_bg = colors[4].into();
                config.ui.status_bar_fg = colors[5].into();
            }
            Self::Color(field, value) => {
                if value.len() != 7
                    || !value.starts_with('#')
                    || !value[1..].bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(SettingsError(
                        "Enter a six-digit hex color, such as #0e639c.",
                    ));
                }
                let target = match field {
                    ColorField::Accent => &mut config.ui.accent_color,
                    ColorField::Text => &mut config.ui.font_color,
                    ColorField::Background => &mut config.ui.bg_primary,
                    ColorField::SecondaryBackground => &mut config.ui.bg_secondary,
                    ColorField::StatusBackground => &mut config.ui.status_bar_bg,
                    ColorField::StatusText => &mut config.ui.status_bar_fg,
                };
                *target = value.to_ascii_lowercase();
            }
            Self::Font { monospace, family } => {
                let family = family.trim();
                if !valid_text(family, 256) {
                    return Err(SettingsError("Choose a font with a nonempty name."));
                }
                if *monospace {
                    config.ui.mono_font_family = family.into();
                } else {
                    config.ui.font_family = family.into();
                }
            }
            Self::EnterpriseSlug(slug) => {
                let slug = slug.trim();
                if !slug.is_empty() && !is_valid_slug(slug) {
                    return Err(SettingsError(
                        "Enter a GitHub enterprise slug or leave it blank.",
                    ));
                }
                config.ui.enterprise_slug = slug.into();
            }
            Self::DashboardCard { id, visible } => {
                if !crate::dashboard::DASHBOARD_CARDS
                    .iter()
                    .any(|card| card.key() == id)
                {
                    return Err(SettingsError("That dashboard card is no longer available."));
                }
                config.set_dashboard_card_visible(id, *visible);
            }
            Self::NotificationEnabled(enabled) => {
                config.notifications.play_sound_on_review_complete = *enabled;
            }
            Self::NotificationSound(path) => {
                let path = path.trim();
                if !path.is_empty() && !supported_sound_path(path) {
                    return Err(SettingsError(
                        "Choose an absolute MP3, WAV, OGG, FLAC, AAC, or M4A file.",
                    ));
                }
                config.notifications.review_complete_sound_path = path.into();
            }
            Self::Provider {
                account_key,
                provider,
            } => {
                let key = account_key.trim().to_lowercase();
                if !config
                    .github
                    .accounts
                    .iter()
                    .any(|account| crate::config::GitHubConfig::override_key(account) == key)
                {
                    return Err(SettingsError(
                        "This account was removed. Reload accounts and try again.",
                    ));
                }
                if *provider == UsageProvider::Codex {
                    let previous: Vec<_> = config
                        .github
                        .accounts
                        .iter()
                        .filter(|account| {
                            config.github.effective_usage_provider(account)
                                == Some(UsageProvider::Codex)
                        })
                        .map(crate::config::GitHubConfig::override_key)
                        .filter(|old| old != &key)
                        .collect();
                    for old in previous {
                        config.github.usage_provider_default_overrides.remove(&old);
                        config
                            .github
                            .usage_provider_overrides
                            .insert(old, UsageProvider::Copilot);
                    }
                }
                config.github.usage_provider_default_overrides.remove(&key);
                config
                    .github
                    .usage_provider_overrides
                    .insert(key, *provider);
            }
        }
        Ok(())
    }
}

/// Each edit contains only the field the user changed, so a Convex mutation
/// and the fresh-file mirror preserve concurrent edits to neighboring fields.
#[derive(Debug, Clone)]
pub enum SyncedEdit {
    RefreshMinutes(u32),
    AutoRefresh(bool),
    RecentlyMergedDays(u32),
    CopilotAccount(String),
    CopilotModel(String),
    PremiumModel(String),
}

impl SyncedEdit {
    pub fn mutation(&self) -> Result<(&'static str, Value), SettingsError> {
        let args = match self {
            Self::RefreshMinutes(value) => {
                if !REFRESH_MINUTES.contains(value) {
                    return Err(SettingsError("Choose a refresh interval from the list."));
                }
                json!({"refreshInterval": value})
            }
            Self::AutoRefresh(value) => json!({"autoRefresh": value}),
            Self::RecentlyMergedDays(value) => {
                if !RECENTLY_MERGED_DAYS.contains(value) {
                    return Err(SettingsError(
                        "Choose a recently merged range from the list.",
                    ));
                }
                json!({"recentlyMergedDays": value})
            }
            Self::CopilotAccount(value) => {
                let value = value.trim();
                if !value.is_empty() && !is_valid_slug(value) {
                    return Err(SettingsError(
                        "Choose a GitHub CLI account or use the active account.",
                    ));
                }
                json!({"ghAccount": value})
            }
            Self::CopilotModel(value) | Self::PremiumModel(value) => {
                let value = value.trim();
                if !valid_text(value, 200) {
                    return Err(SettingsError("Enter a model ID of 1 to 200 characters."));
                }
                if matches!(self, Self::CopilotModel(_)) {
                    json!({"model": value})
                } else {
                    json!({"premiumModel": value})
                }
            }
        };
        let function = match self {
            Self::RefreshMinutes(_) | Self::AutoRefresh(_) | Self::RecentlyMergedDays(_) => {
                "settings:updatePR"
            }
            _ => "settings:updateCopilot",
        };
        Ok((function, args))
    }

    /// Apply only after the corresponding Convex mutation succeeded. This is
    /// a read fallback, never a substitute for a disconnected backend write.
    pub fn apply_mirror(&self, config: &mut AppConfig) -> Result<(), SettingsError> {
        self.mutation()?;
        match self {
            Self::RefreshMinutes(value) => config.pr.refresh_interval = f64::from(*value),
            Self::AutoRefresh(value) => config.pr.auto_refresh = *value,
            Self::RecentlyMergedDays(value) => config.pr.recently_merged_days = f64::from(*value),
            Self::CopilotAccount(value) => config.copilot.gh_account = value.trim().into(),
            Self::CopilotModel(value) => config.copilot.model = value.trim().into(),
            Self::PremiumModel(value) => config.copilot.premium_model = value.trim().into(),
        }
        Ok(())
    }
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub fn supported_sound_path(value: &str) -> bool {
    let path = Path::new(value);
    value.len() <= 4096
        && path.is_absolute()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["mp3", "wav", "ogg", "flac", "aac", "m4a"]
                    .contains(&extension.to_ascii_lowercase().as_str())
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_edits_leave_all_fields_untouched() {
        let mut config = AppConfig::default();
        let original = config.clone();
        for edit in [
            LocalEdit::Color(ColorField::Accent, "#nothex".into()),
            LocalEdit::Font {
                monospace: false,
                family: "\n".into(),
            },
            LocalEdit::EnterpriseSlug("wrong/slug".into()),
            LocalEdit::Provider {
                account_key: "org/missing".into(),
                provider: UsageProvider::Codex,
            },
            LocalEdit::NotificationSound("relative.mp3".into()),
        ] {
            assert!(edit.apply(&mut config).is_err());
            assert_eq!(config, original);
        }
    }

    #[test]
    fn provider_transfer_uses_fresh_effective_accounts_and_removes_seeded_defaults() {
        let mut config = AppConfig::from_json(
            r#"{"github":{"accounts":[
            {"org":"one","username":"first","usageProvider":"codex"},
            {"org":"two","username":"second"}],
            "usageProviderOverrides":{"one/first":"codex"},
            "usageProviderDefaultOverrides":{"one/first":"codex"}}}"#,
        )
        .unwrap();
        LocalEdit::Provider {
            account_key: "TWO/SECOND".into(),
            provider: UsageProvider::Codex,
        }
        .apply(&mut config)
        .unwrap();
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[0]),
            Some(UsageProvider::Copilot)
        );
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[1]),
            Some(UsageProvider::Codex)
        );
        assert!(config.github.usage_provider_default_overrides.is_empty());
    }

    #[test]
    fn partial_edits_preserve_concurrent_settings_unknown_fields_and_prompt_template() {
        let mut config = AppConfig::from_json(
            r##"{"ui":{"accentColor":"#112233","future":4},
            "pr":{"autoRefresh":false,"future":5},
            "copilot":{"model":"other-model","prReviewPromptTemplate":"keep {{prUrl}}","future":6},
            "notifications":{"future":7},"unmodeled":{"keep":true}}"##,
        )
        .unwrap();
        SyncedEdit::RefreshMinutes(30)
            .apply_mirror(&mut config)
            .unwrap();
        SyncedEdit::PremiumModel(" custom/model ".into())
            .apply_mirror(&mut config)
            .unwrap();
        LocalEdit::NotificationEnabled(true)
            .apply(&mut config)
            .unwrap();
        let result = serde_json::to_value(&config).unwrap();
        assert_eq!(result["ui"]["accentColor"], "#112233");
        assert_eq!(result["ui"]["future"], 4);
        assert_eq!(result["pr"]["autoRefresh"], false);
        assert_eq!(result["pr"]["future"], 5);
        assert_eq!(result["copilot"]["model"], "other-model");
        assert_eq!(result["copilot"]["premiumModel"], "custom/model");
        assert_eq!(
            result["copilot"]["prReviewPromptTemplate"],
            "keep {{prUrl}}"
        );
        assert_eq!(result["copilot"]["future"], 6);
        assert_eq!(result["notifications"]["future"], 7);
        assert_eq!(result["unmodeled"]["keep"], true);
    }

    #[test]
    fn mutations_send_only_the_changed_field_and_reject_invalid_values() {
        assert_eq!(
            SyncedEdit::AutoRefresh(false).mutation().unwrap(),
            ("settings:updatePR", json!({"autoRefresh": false}))
        );
        assert_eq!(
            SyncedEdit::CopilotAccount("".into()).mutation().unwrap(),
            ("settings:updateCopilot", json!({"ghAccount": ""}))
        );
        assert!(SyncedEdit::RefreshMinutes(2).mutation().is_err());
        assert!(SyncedEdit::RecentlyMergedDays(0).mutation().is_err());
        assert!(SyncedEdit::CopilotModel("\t".into()).mutation().is_err());
        assert!(
            SyncedEdit::CopilotAccount("bad/account".into())
                .mutation()
                .is_err()
        );
    }

    #[test]
    fn theme_selection_applies_incumbent_presets_without_losing_other_preferences() {
        let mut config = AppConfig::default();
        config.ui.font_family = "fixture font".into();
        config.ui.extra.insert("future".into(), json!(42));
        LocalEdit::Theme(ThemeName::Light)
            .apply(&mut config)
            .unwrap();
        assert_eq!(
            [
                config.ui.accent_color.as_str(),
                config.ui.font_color.as_str(),
                config.ui.bg_primary.as_str(),
                config.ui.bg_secondary.as_str(),
                config.ui.status_bar_bg.as_str(),
                config.ui.status_bar_fg.as_str()
            ],
            [
                "#0078d4", "#1f1f1f", "#ffffff", "#f3f3f3", "#f3f3f3", "#616161"
            ]
        );
        assert_eq!(config.ui.font_family, "fixture font");
        assert_eq!(config.ui.extra["future"], 42);
        LocalEdit::Theme(ThemeName::Dark)
            .apply(&mut config)
            .unwrap();
        assert_eq!(config.ui.bg_primary, "#1e1e1e");
    }

    #[test]
    fn reset_preserves_unrelated_ui_and_all_synced_preferences() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        let preserved = json!({
            "enterpriseSlug": "keep-enterprise", "favoriteUsers": ["keep-user"],
            "zoomLevel": 125, "sidebarWidth": 420, "paneSizes": [200, 800],
            "showBookmarkedOnly": true, "terminalOpen": true, "assistantOpen": true,
            "terminalPanelHeight": 450, "displayId": 23,
            "displayBounds": {"x": 10, "y": 20, "width": 1600, "height": 900},
            "displayWorkArea": {"x": 10, "y": 20, "width": 1600, "height": 860},
            "weatherLocation": {"latitude": 40, "longitude": -74, "name": "Keep city"},
            "futurePreference": {"keep": true}
        });
        for (key, entry) in preserved.as_object().unwrap() {
            value["ui"][key] = entry.clone();
        }
        value["ui"]["accentColor"] = json!("#abcdef");
        value["ui"]["fontFamily"] = json!("Other font");
        value["ui"]["dashboardCards"] = json!({"command-center": false});
        value["pr"]["refreshInterval"] = json!(30);
        value["copilot"]["model"] = json!("keep-model");
        value["notifications"]["future"] = json!(42);
        value["notifications"]["reviewCompleteSoundPath"] = json!("old.wav");
        value["github"]["usageProviderOverrides"] = json!({"team/user": "codex"});
        let mut config: AppConfig = serde_json::from_value(value).unwrap();
        let before = serde_json::to_value(&config).unwrap();
        reset_local_preferences(&mut config);
        let result = serde_json::to_value(&config).unwrap();
        for key in preserved.as_object().unwrap().keys() {
            assert_eq!(result["ui"][key], before["ui"][key], "{key}");
        }
        assert_eq!(result["pr"], before["pr"]);
        assert_eq!(result["copilot"], before["copilot"]);
        assert_eq!(result["notifications"]["future"], 42);
        assert_eq!(config.ui.accent_color, AppConfig::default().ui.accent_color);
        assert_eq!(config.ui.font_family, AppConfig::default().ui.font_family);
        assert_eq!(
            config.ui.dashboard_cards,
            AppConfig::default().ui.dashboard_cards
        );
        assert!(config.notifications.review_complete_sound_path.is_empty());
        assert!(config.github.usage_provider_overrides.is_empty());
    }
}
