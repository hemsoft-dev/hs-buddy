//! Electron's prompt-box "Recent" list and active-prompt badge. Both are read
//! once per refresh; nothing here writes.
use buddy_core::copilot_backend::{ActiveCounts, CopilotBackend, Error, HistoryFilter};
use buddy_core::copilot_results::status_emoji;
use buddy_core::copilot_results::summaries::ResultSummary;

/// `useCopilotResultsRecent(10)` in `CopilotPromptBox.tsx`.
pub(super) const LIMIT: u8 = 10;
const PROMPT_CHARS: usize = 80;

#[derive(Default)]
pub(super) struct Recent {
    pub rows: Vec<ResultSummary>,
    pub active: ActiveCounts,
    pub loading: bool,
    /// Another refresh was asked for while one was loading.
    pub stale: bool,
    pub unavailable: bool,
}

pub(super) struct Fetched {
    rows: Result<Vec<ResultSummary>, Error>,
    active: Result<ActiveCounts, Error>,
}

pub(super) async fn fetch(backend: CopilotBackend) -> Fetched {
    let rows = backend
        .history(None, LIMIT, &HistoryFilter::All)
        .await
        .map(|page| page.rows);
    let active = backend.count_active().await;
    Fetched { rows, active }
}

impl Recent {
    /// A failed read keeps the last rows and count rather than showing none.
    pub fn apply(&mut self, fetched: Option<Fetched>) {
        self.loading = false;
        let Some(fetched) = fetched else {
            self.unavailable = true;
            return;
        };
        self.unavailable = fetched.rows.is_err() || fetched.active.is_err();
        if let Ok(rows) = fetched.rows {
            self.rows = rows;
            self.rows.truncate(LIMIT.into());
        }
        if let Ok(active) = fetched.active {
            self.active = active;
        }
    }

    /// `{totalActive} active`, shown only while something is pending or running.
    pub fn badge(&self) -> Option<String> {
        let total = self.active.total();
        (total > 0).then(|| format!("{total} active"))
    }
}

/// Status emoji, the prompt cut at 80 characters, and the relative time.
pub(super) fn line(row: &ResultSummary, now_ms: f64) -> String {
    let mut prompt: String = row.prompt_preview.chars().take(PROMPT_CHARS).collect();
    if row.prompt_preview.chars().nth(PROMPT_CHARS).is_some() {
        prompt.push('…');
    }
    let elapsed = (now_ms - row.created_at).max(0.0) as u64;
    format!(
        "{} {prompt} · {}",
        status_emoji(row.status),
        buddy_core::format::distance_to_now(elapsed)
    )
}

pub(super) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

#[cfg(test)]
pub(super) fn fetched(rows: Vec<ResultSummary>, active: Option<ActiveCounts>) -> Fetched {
    Fetched {
        rows: Ok(rows),
        active: active.ok_or(Error::Unavailable),
    }
}
