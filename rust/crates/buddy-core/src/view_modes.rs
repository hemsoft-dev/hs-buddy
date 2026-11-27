//! Card / list choices per page, as Electron's `useViewMode` keeps them: a local copy that
//! wins (config.json `native.<section>.viewModes.<page key>` here, localStorage
//! `viewMode:<page key>` in Electron) and Convex `settings.viewModes`, written on every
//! change and adopted only for a page with no local choice yet.
use std::collections::BTreeMap;

use serde_json::json;

pub use crate::github::ViewMode;

use crate::config::AppConfig;
use crate::settings_backend::BackendError;

/// The local choice for `page_key`, or `None` when this device has none.
pub fn local(config: &AppConfig, section: &str, page_key: &str) -> Option<ViewMode> {
    config.native.extra.get(section)?["viewModes"]
        .get(page_key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

/// Records `view` for `page_key`, keeping every other key of the section.
pub fn set_local(config: &mut AppConfig, section: &str, page_key: &str, view: ViewMode) {
    let entry = config
        .native
        .extra
        .entry(section.into())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    if !entry["viewModes"].is_object() {
        entry["viewModes"] = json!({});
    }
    entry["viewModes"][page_key] = json!(view);
}

/// Every synced choice (`settings:get`'s `viewModes`).
pub async fn load(url: &str) -> Result<BTreeMap<String, ViewMode>, BackendError> {
    let value = crate::github::backend::query(url, "settings:get").await?;
    serde_json::from_value(value.get("viewModes").cloned().unwrap_or_else(|| json!({})))
        .map_err(|_| BackendError::InvalidSnapshot)
}

/// Syncs one page's choice, as Electron's toggle does (`settings:updateViewMode`).
pub async fn save(url: &str, page_key: &str, view: ViewMode) -> Result<(), BackendError> {
    crate::github::backend::mutate(
        url,
        "settings:updateViewMode",
        json!({"pageKey":page_key,"mode":view}),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_choice_keeps_the_sections_other_keys_and_pages() {
        let mut config = AppConfig::default();
        assert_eq!(local(&config, "automation", "run-list"), None);
        config.native.extra.insert(
            "automation".into(),
            json!({"future":42,"viewModes":{"other":"list"}}),
        );
        set_local(&mut config, "automation", "run-list", ViewMode::List);
        assert_eq!(
            local(&config, "automation", "run-list"),
            Some(ViewMode::List)
        );
        assert_eq!(config.native.extra["automation"]["future"], 42);
        assert_eq!(
            config.native.extra["automation"]["viewModes"]["other"],
            "list"
        );
        set_local(&mut config, "automation", "run-list", ViewMode::Card);
        assert_eq!(
            config.native.extra["automation"]["viewModes"]["run-list"],
            "card"
        );
    }

    #[test]
    fn a_malformed_section_or_value_reads_as_no_choice_and_is_replaced_on_write() {
        let mut config = AppConfig::default();
        for stored in [
            json!(7),
            json!({"viewModes":3}),
            json!({"viewModes":{"run-list":"grid"}}),
        ] {
            config.native.extra.insert("automation".into(), stored);
            assert_eq!(local(&config, "automation", "run-list"), None);
            set_local(&mut config, "automation", "run-list", ViewMode::List);
            assert_eq!(
                local(&config, "automation", "run-list"),
                Some(ViewMode::List)
            );
        }
    }
}
