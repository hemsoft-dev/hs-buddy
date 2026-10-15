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
            Self::Theme(theme) => config.ui.theme = *theme,
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
}
