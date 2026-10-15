//! The incumbent Convex bookmark and view-preference contracts.
use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{PrMode, RepositoryRef, ViewMode};
use crate::config::AppConfig;
use crate::settings_backend::{BackendError, connect, result_json};

const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Deserialize)]
pub struct Bookmark {
    #[serde(rename = "_id")]
    pub id: String,
    pub owner: String,
    pub repo: String,
    pub folder: String,
}

impl Bookmark {
    pub fn target(&self) -> Result<RepositoryRef, super::GitHubError> {
        RepositoryRef::new(&self.owner, &self.repo)
    }
}

/// Local state wins; a missing key may adopt the existing synced preference.
pub fn view_mode(config: &AppConfig, mode: PrMode) -> Option<ViewMode> {
    config.native.extra.get("github")?["viewModes"]
        .get(page_key(mode))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub fn set_view_mode(config: &mut AppConfig, mode: PrMode, view: ViewMode) {
    let github = config
        .native
        .extra
        .entry("github".into())
        .or_insert_with(|| json!({}));
    if !github.is_object() {
        *github = json!({});
    }
    if !github["viewModes"].is_object() {
        github["viewModes"] = json!({});
    }
    github["viewModes"][page_key(mode)] = json!(view);
}

pub fn page_key(mode: PrMode) -> String {
    format!("pr-list-{}", mode.key())
}

pub async fn load_view_modes(url: &str) -> Result<BTreeMap<String, ViewMode>, BackendError> {
    let value = query(url, "settings:get").await?;
    serde_json::from_value(value.get("viewModes").cloned().unwrap_or_else(|| json!({})))
        .map_err(|_| BackendError::InvalidSnapshot)
}

pub async fn save_view_mode(url: &str, mode: PrMode, view: ViewMode) -> Result<(), BackendError> {
    mutate(
        url,
        "settings:updateViewMode",
        json!({"pageKey":page_key(mode),"mode":view}),
    )
    .await?;
    Ok(())
}

pub async fn bookmarks(url: &str) -> Result<Vec<Bookmark>, BackendError> {
    let bookmarks: Vec<Bookmark> = serde_json::from_value(query(url, "repoBookmarks:list").await?)
        .map_err(|_| BackendError::InvalidSnapshot)?;
    if bookmarks
        .iter()
        .any(|item| item.id.is_empty() || item.target().is_err())
    {
        return Err(BackendError::InvalidSnapshot);
    }
    Ok(bookmarks)
}

pub struct BookmarkReceipt {
    pub stats_updated: bool,
}

pub async fn add_bookmark(
    url: &str,
    target: &RepositoryRef,
    folder: &str,
) -> Result<BookmarkReceipt, BackendError> {
    let result = mutate(
        url,
        "repoBookmarks:create",
        json!({"folder":folder,"owner":target.owner(),"repo":target.name(),"url":target.web_url()}),
    )
    .await?;
    if result["_id"].as_str().is_none_or(str::is_empty) || !result["inserted"].is_boolean() {
        return Err(BackendError::UnknownSaveResult);
    }
    // A statistics failure cannot undo the acknowledged bookmark or invite a retry.
    let stats_updated = !result["inserted"].as_bool().unwrap_or(false)
        || mutate(
            url,
            "buddyStats:increment",
            json!({"field":"bookmarksCreated"}),
        )
        .await
        .is_ok();
    Ok(BookmarkReceipt { stats_updated })
}

pub async fn remove_bookmark(url: &str, id: &str) -> Result<(), BackendError> {
    if id.is_empty() {
        return Err(BackendError::InvalidSnapshot);
    }
    let result = mutate(url, "repoBookmarks:remove", json!({"id":id})).await?;
    if result.as_str() != Some(id) {
        return Err(BackendError::UnknownSaveResult);
    }
    Ok(())
}

async fn query(url: &str, function: &str) -> Result<Value, BackendError> {
    let mut client = connect(url).await?;
    let result = tokio::time::timeout(TIMEOUT, client.query(function, BTreeMap::new()))
        .await
        .map_err(|_| BackendError::Unavailable)?
        .map_err(|_| BackendError::Unavailable)?;
    result_json(result)
}

async fn mutate(url: &str, function: &str, arguments: Value) -> Result<Value, BackendError> {
    let args = arguments
        .as_object()
        .ok_or(BackendError::InvalidSnapshot)?
        .iter()
        .map(|(key, value)| {
            convex::Value::try_from(value.clone())
                .map(|value| (key.clone(), value))
                .map_err(|_| BackendError::InvalidSnapshot)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut client = connect(url).await?;
    let result = tokio::time::timeout(TIMEOUT, client.mutation(function, args))
        .await
        .map_err(|_| BackendError::UnknownSaveResult)?
        .map_err(|_| BackendError::UnknownSaveResult)?;
    result_json(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_mode_edit_preserves_other_preferences() {
        let mut config = AppConfig::default();
        config.native.extra.insert(
            "github".into(),
            json!({"future":42,"viewModes":{"unrelated":"list"}}),
        );
        set_view_mode(&mut config, PrMode::MyPrs, ViewMode::List);
        assert_eq!(view_mode(&config, PrMode::MyPrs), Some(ViewMode::List));
        assert_eq!(config.native.extra["github"]["future"], 42);
        assert_eq!(
            config.native.extra["github"]["viewModes"]["unrelated"],
            "list"
        );
    }
}
