//! Bounded cursor pages from `copilotNative`. Split-required pages cannot be
//! appended as a complete history range; the caller must fetch both halves.
use serde::Deserialize;
use serde_json::Value;

use super::{DataError, Status};

const MAX_BYTES: usize = 128 * 1024;
const MAX_CURSOR: usize = 16 * 1024;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSummary {
    #[serde(rename = "_id")]
    pub id: String,
    pub prompt_preview: String,
    pub status: Status,
    pub model_label: Option<String>,
    pub category_label: Option<String>,
    pub created_at: f64,
    pub completed_at: Option<f64>,
    pub duration: Option<f64>,
    pub has_output: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    #[serde(rename = "_id")]
    pub id: String,
    pub result_id: String,
    pub title_preview: String,
    pub status: Status,
    pub model_label: Option<String>,
    pub account_label: Option<String>,
    pub reviewed_head_sha: Option<String>,
    pub reviewed_thread_stats: Option<ThreadStats>,
    pub created_at: f64,
    pub updated_at: f64,
    pub completed_at: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ThreadStats {
    #[serde(deserialize_with = "count")]
    pub total: u32,
    #[serde(deserialize_with = "count")]
    pub unresolved: u32,
    #[serde(deserialize_with = "count")]
    pub outdated: u32,
}
fn count<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let number = f64::deserialize(deserializer)?;
    if !number.is_finite() || number.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&number) {
        return Err(serde::de::Error::custom("Invalid review thread count"));
    }
    Ok(number as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SplitStatus {
    SplitRecommended,
    SplitRequired,
    #[serde(other)]
    Unsupported,
}

/// No Debug: previews and opaque cursors can contain private data.
pub struct Page<T> {
    pub rows: Vec<T>,
    pub skipped: usize,
    pub is_done: bool,
    pub continue_cursor: String,
    pub split_cursor: Option<String>,
    pub split_status: Option<SplitStatus>,
}

impl<T> Page<T> {
    pub fn can_append(&self) -> bool {
        !matches!(
            self.split_status,
            Some(SplitStatus::SplitRequired | SplitStatus::Unsupported)
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePage {
    page: Vec<Value>,
    is_done: bool,
    continue_cursor: String,
    split_cursor: Option<String>,
    page_status: Option<SplitStatus>,
}

fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn label(value: &str) -> bool {
    value.chars().take(202).count() <= 201 && !value.chars().any(char::is_control)
}

fn time(value: f64) -> bool {
    value.is_finite() && (0.0..=253_402_300_799_999.0).contains(&value)
}

fn result_valid(row: &ResultSummary) -> bool {
    id(&row.id)
        && label(&row.prompt_preview)
        && row.model_label.as_deref().is_none_or(label)
        && row.category_label.as_deref().is_none_or(label)
        && time(row.created_at)
        && row.completed_at.is_none_or(time)
        && row.duration.is_none_or(time)
}

fn review_valid(row: &ReviewSummary) -> bool {
    id(&row.id)
        && id(&row.result_id)
        && label(&row.title_preview)
        && row.model_label.as_deref().is_none_or(label)
        && row.account_label.as_deref().is_none_or(label)
        && row.reviewed_head_sha.as_ref().is_none_or(|head| {
            matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && row
            .reviewed_thread_stats
            .is_none_or(|stats| stats.unresolved <= stats.total && stats.outdated <= stats.total)
        && time(row.created_at)
        && time(row.updated_at)
        && row.completed_at.is_none_or(time)
}

fn parse<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    valid: fn(&T) -> bool,
) -> Result<Page<T>, DataError> {
    if bytes.len() > MAX_BYTES {
        return Err(DataError::TooLarge);
    }
    let wire: WirePage = serde_json::from_slice(bytes).map_err(|_| DataError::InvalidResponse)?;
    if wire.page.len() > 25 {
        return Err(DataError::TooManyResults);
    }
    if wire.continue_cursor.len() > MAX_CURSOR
        || (!wire.is_done && wire.continue_cursor.is_empty())
        || wire
            .split_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR)
        || (matches!(
            wire.page_status,
            Some(SplitStatus::SplitRecommended | SplitStatus::SplitRequired)
        ) && wire.split_cursor.is_none())
    {
        return Err(DataError::InvalidResponse);
    }
    let mut page = Page {
        rows: Vec::new(),
        skipped: 0,
        is_done: wire.is_done,
        continue_cursor: wire.continue_cursor,
        split_cursor: wire.split_cursor,
        split_status: wire.page_status,
    };
    for value in wire.page {
        match serde_json::from_value(value) {
            Ok(row) if valid(&row) => page.rows.push(row),
            _ => page.skipped += 1,
        }
    }
    Ok(page)
}

pub fn parse_results(bytes: &[u8]) -> Result<Page<ResultSummary>, DataError> {
    parse(bytes, result_valid)
}

pub fn parse_reviews(bytes: &[u8]) -> Result<Page<ReviewSummary>, DataError> {
    parse(bytes, review_valid)
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
