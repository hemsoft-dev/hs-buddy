//! Electron's prompt-box "Recent" list, its sidebar "Recent Results" section
//! and the active-prompt badge. One read serves all three; nothing here writes.
use buddy_core::copilot_backend::{ActiveCounts, CopilotBackend, Error, HistoryFilter};
use buddy_core::copilot_results::Status;
use buddy_core::copilot_results::status_emoji;
use buddy_core::copilot_results::summaries::ResultSummary;

/// `useCopilotResultsRecent(10)` in `CopilotPromptBox.tsx`.
pub(super) const LIMIT: u8 = 10;
/// `useCopilotResultsRecent(15)` in `CopilotSidebar.tsx`.
pub(super) const SIDEBAR_LIMIT: u8 = 15;
const PROMPT_CHARS: usize = 80;
/// `getResultPromptLabel` in `CopilotSidebar.tsx`.
const SIDEBAR_PROMPT_CHARS: usize = 40;

#[derive(Default)]
pub(super) struct Recent {
    /// The newest results, up to [`SIDEBAR_LIMIT`]; the prompt box lists the
    /// first [`LIMIT`].
    pub rows: Vec<ResultSummary>,
    /// The last count read, or `None` while it is unknown: never read, or its
    /// latest read failed. Electron shows no count it does not have.
    pub active: Option<ActiveCounts>,
    pub loading: bool,
    /// Another refresh was asked for while one was loading.
    pub stale: bool,
    /// The rows could not be read again, so the list shown is the last one.
    /// The active count has no bearing on it.
    pub unavailable: bool,
    /// The read stopped early (a split page), so newer results may be missing
    /// from the prompt box's ten.
    pub partial: bool,
    /// The same for the sidebar's fifteen.
    pub sidebar_partial: bool,
}

pub(super) struct Fetched {
    rows: Result<(Vec<ResultSummary>, bool, bool), Error>,
    active: Result<ActiveCounts, Error>,
}

pub(super) async fn fetch(backend: CopilotBackend) -> Fetched {
    let rows = backend
        .history(None, SIDEBAR_LIMIT, &HistoryFilter::All)
        .await
        .map(|page| {
            let partial = |limit: u8| {
                !page.can_append() || (!page.is_done && page.rows.len() < usize::from(limit))
            };
            let (prompt, sidebar) = (partial(LIMIT), partial(SIDEBAR_LIMIT));
            (page.rows, prompt, sidebar)
        });
    let active = backend.count_active().await;
    Fetched { rows, active }
}

impl Recent {
    /// A failed rows read keeps the last rows, marked as not refreshed. A
    /// failed count read leaves fresh rows fresh and only the count unknown.
    pub fn apply(&mut self, fetched: Option<Fetched>) {
        self.loading = false;
        let Some(fetched) = fetched else {
            self.unavailable = true;
            self.active = None;
            return;
        };
        match fetched.rows {
            Ok((rows, partial, sidebar_partial)) => {
                self.rows = rows;
                self.rows.truncate(SIDEBAR_LIMIT.into());
                self.partial = partial;
                self.sidebar_partial = sidebar_partial;
                self.unavailable = false;
            }
            Err(_) => self.unavailable = true,
        }
        self.active = fetched.active.ok();
    }

    /// `{totalActive} active`, shown only while something is known to be
    /// pending or running.
    pub fn badge(&self) -> Option<String> {
        let total = self.active?.total();
        (total > 0).then(|| format!("{total} active"))
    }

    /// The prompt box's ten.
    pub fn prompt_rows(&self) -> impl Iterator<Item = &ResultSummary> {
        self.rows.iter().take(LIMIT.into())
    }
}

/// `getRecentResultLabel`: a pr-review result shows its PR title, any other
/// its prompt cut at 40 characters.
pub(super) fn sidebar_label(row: &ResultSummary) -> String {
    if let Some(title) = row.review_title() {
        let title = if title.is_empty() { "Untitled" } else { title };
        return format!("PR Review: {title}");
    }
    let mut prompt: String = row
        .prompt_preview
        .chars()
        .take(SIDEBAR_PROMPT_CHARS)
        .collect();
    if row
        .prompt_preview
        .chars()
        .nth(SIDEBAR_PROMPT_CHARS)
        .is_some()
    {
        prompt.push('…');
    }
    prompt
}

/// `getStatusIcon`: Electron's lucide icon for each status, and none for a
/// status it does not know.
pub(super) fn status_icon(status: Status) -> Option<gpui_kit::assets::IconName> {
    use gpui_kit::assets::IconName;
    Some(match status {
        Status::Pending => IconName::Clock,
        Status::Running => IconName::LoaderCircle,
        Status::Completed => IconName::CircleCheck,
        Status::Failed => IconName::CircleX,
        Status::Unknown => return None,
    })
}

/// `formatDistanceToNow(createdAt)`.
pub(super) fn age(row: &ResultSummary, now_ms: f64) -> String {
    let elapsed = (now_ms - row.created_at).max(0.0) as u64;
    buddy_core::format::distance_to_now(elapsed)
}

/// Status emoji, the prompt cut at 80 characters, and the relative time.
pub(super) fn line(row: &ResultSummary, now_ms: f64) -> String {
    let mut prompt: String = row.prompt_preview.chars().take(PROMPT_CHARS).collect();
    if row.prompt_preview.chars().nth(PROMPT_CHARS).is_some() {
        prompt.push('…');
    }
    format!(
        "{} {prompt} · {}",
        status_emoji(row.status),
        age(row, now_ms)
    )
}

pub(super) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

#[cfg(test)]
pub(super) fn fetched(rows: Vec<ResultSummary>, active: Option<ActiveCounts>) -> Fetched {
    fetched_page(rows, false, active)
}

#[cfg(test)]
pub(super) fn failed_rows(active: Option<ActiveCounts>) -> Fetched {
    Fetched {
        rows: Err(Error::Unavailable),
        active: active.ok_or(Error::Unavailable),
    }
}

#[cfg(test)]
pub(super) fn fetched_page(
    rows: Vec<ResultSummary>,
    partial: bool,
    active: Option<ActiveCounts>,
) -> Fetched {
    Fetched {
        rows: Ok((rows, partial, partial)),
        active: active.ok_or(Error::Unavailable),
    }
}
