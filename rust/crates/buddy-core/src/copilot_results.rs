//! Read-only adapters for existing Convex result records. No whole-record writes.
use serde::Deserialize;
use serde_json::Value;

#[path = "copilot_results/summaries.rs"]
pub mod summaries;

const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Running,
    Completed,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentState {
    Pending,
    Running,
    Captured,
    Missing,
    Failed,
    Unknown,
}

/// Prompt, result, error and metadata can contain private user data. No Debug.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedResult {
    #[serde(rename = "_id")]
    pub id: String,
    pub prompt: String,
    pub status: Status,
    pub result: Option<String>,
    pub error: Option<String>,
    pub model: Option<String>,
    pub category: Option<String>,
    pub metadata: Option<Value>,
    pub duration: Option<f64>,
    pub created_at: f64,
    pub completed_at: Option<f64>,
}

impl SavedResult {
    pub fn content_state(&self) -> ContentState {
        match self.status {
            Status::Pending => ContentState::Pending,
            Status::Running => ContentState::Running,
            Status::Failed => ContentState::Failed,
            Status::Unknown => ContentState::Unknown,
            Status::Completed if self.result.is_some() => ContentState::Captured,
            Status::Completed => ContentState::Missing,
        }
    }

    pub fn prompt_preview(&self) -> String {
        self.prompt.chars().take(200).collect()
    }

    fn valid(&self) -> bool {
        let short = |text: &String| text.len() <= 256 && !text.chars().any(char::is_control);
        let timestamp =
            |time: f64| time.is_finite() && (0.0..=253_402_300_799_999.0).contains(&time);
        !self.id.is_empty()
            && short(&self.id)
            // Existing records can predate the native execution prompt cap.
            && self.prompt.len() <= 1024 * 1024
            && self
                .result
                .as_ref()
                .is_none_or(|text| text.len() <= 4 * 1024 * 1024)
            && self
                .error
                .as_ref()
                .is_none_or(|text| text.len() <= 1024 * 1024)
            && self.model.as_ref().is_none_or(short)
            && self.category.as_ref().is_none_or(short)
            && timestamp(self.created_at)
            && self.completed_at.is_none_or(timestamp)
            && self.duration.is_none_or(timestamp)
    }
}

pub struct HistoryPage {
    pub results: Vec<SavedResult>,
    /// Malformed/oversized rows are reported, never silently presented as an empty success.
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataError {
    TooLarge,
    InvalidResponse,
    TooManyResults,
    InvalidRecord,
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "Result data exceeds the response limit.",
            Self::InvalidResponse => "The result service returned an unsupported response.",
            Self::TooManyResults => "The result service returned too many records.",
            Self::InvalidRecord => "This result record has invalid or oversized fields.",
        })
    }
}

impl std::error::Error for DataError {}

fn response(bytes: &[u8]) -> Result<Value, DataError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(DataError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| DataError::InvalidResponse)
}

fn record(value: Value) -> Result<SavedResult, DataError> {
    let record: SavedResult =
        serde_json::from_value(value).map_err(|_| DataError::InvalidRecord)?;
    if !record.valid() {
        return Err(DataError::InvalidRecord);
    }
    Ok(record)
}

pub fn parse_history(bytes: &[u8]) -> Result<HistoryPage, DataError> {
    let Value::Array(rows) = response(bytes)? else {
        return Err(DataError::InvalidResponse);
    };
    if rows.len() > MAX_ROWS {
        return Err(DataError::TooManyResults);
    }
    let mut page = HistoryPage {
        results: Vec::new(),
        skipped: 0,
    };
    for row in rows {
        match record(row) {
            Ok(result) => page.results.push(result),
            Err(_) => page.skipped += 1,
        }
    }
    Ok(page)
}

pub fn parse_detail(bytes: &[u8]) -> Result<Option<SavedResult>, DataError> {
    match response(bytes)? {
        Value::Null => Ok(None),
        value => record(value).map(Some),
    }
}

#[cfg(test)]
#[path = "copilot_results/tests.rs"]
mod tests;
