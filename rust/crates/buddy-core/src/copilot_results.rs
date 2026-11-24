//! Read-only adapters for existing Convex result records. No whole-record writes.
use serde::Deserialize;
use serde_json::Value;

pub mod publish;
#[path = "copilot_results/summaries.rs"]
pub mod summaries;

const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_LINK: usize = 2048;
pub const REVIEW_CATEGORY: &str = "pr-review";

/// Electron's prompt-box categories, stored on a saved result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    PrReview,
    CodeAnalysis,
    Documentation,
}

impl Category {
    pub const ALL: [Self; 4] = [
        Self::General,
        Self::PrReview,
        Self::CodeAnalysis,
        Self::Documentation,
    ];
    /// The value stored on the result record.
    pub fn code(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::PrReview => REVIEW_CATEGORY,
            Self::CodeAnalysis => "code-analysis",
            Self::Documentation => "documentation",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::PrReview => "PR Review",
            Self::CodeAnalysis => "Code Analysis",
            Self::Documentation => "Documentation",
        }
    }
}

/// `getStatusEmoji` from Electron's `statusDisplay.ts`.
pub fn status_emoji(status: Status) -> &'static str {
    match status {
        Status::Pending => "⏳",
        Status::Running => "🔄",
        Status::Completed => "✅",
        Status::Failed => "❌",
        Status::Unknown => "•",
    }
}

/// A stored link opens only as an http(s) URL with a host and no credentials.
pub fn web_link(value: &str) -> Option<String> {
    if value.len() > MAX_LINK || value.chars().any(char::is_control) {
        return None;
    }
    let parsed = url::Url::parse(value).ok()?;
    (matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some_and(|host| !host.is_empty())
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then(|| parsed.to_string())
}
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
    pub fn review_pull_request_number(&self) -> Option<u64> {
        // Convex numbers arrive through the SDK as Float64, including whole
        // PR numbers. Accept exact positive JS integers without rounding.
        let number = self.metadata.as_ref()?.get("prNumber")?.as_f64()?;
        (number.is_finite()
            && number.fract() == 0.0
            && (1.0..=9_007_199_254_740_991.0).contains(&number))
        .then_some(number as u64)
    }
    /// Electron offers "Open PR on GitHub" only for a pr-review result with
    /// a metadata.prUrl. Native opens it only as a credential-free web link.
    pub fn pull_request_url(&self) -> Option<String> {
        if self.category.as_deref() != Some(REVIEW_CATEGORY) {
            return None;
        }
        web_link(self.metadata.as_ref()?.get("prUrl")?.as_str()?)
    }
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

    /// Electron re-runs with the saved metadata's `ghAccount`.
    pub fn saved_account(&self) -> Option<&str> {
        self.metadata
            .as_ref()?
            .get("ghAccount")?
            .as_str()
            .map(str::trim)
            .filter(|account| !account.is_empty())
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
