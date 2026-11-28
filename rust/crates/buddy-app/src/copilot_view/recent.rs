//! Electron's prompt-box "Recent" list, its sidebar "Recent Results" section
//! and the active-prompt badge. One read serves all three; nothing here writes.
use buddy_core::copilot_backend::{ActiveCounts, CopilotBackend, Error, HistoryFilter};
use buddy_core::copilot_results::Status;
use buddy_core::copilot_results::status_emoji;
use buddy_core::copilot_results::summaries::ResultSummary;
use gpui_kit::Hsla;
use gpui_kit::component::theme::Theme;

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
    /// Bumped by every read that starts and every local removal. A reply
    /// applies only if it carries the current one, so a read that was in
    /// flight when a result went away cannot list it again.
    generation: u64,
    /// Results known to be gone, each with the generation current when it
    /// went. A reply from a read started at or before that generation never
    /// lists it; the first successful read started after it drops the entry.
    removed: Vec<(String, u64)>,
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
    /// Starts a read: marks the list loading and returns the generation the
    /// read's reply must carry to be applied.
    pub fn begin(&mut self) -> u64 {
        self.loading = true;
        self.stale = false;
        self.generation += 1;
        self.generation
    }

    /// Applies the reply of the read started at `generation`, and returns
    /// whether it applied. A reply from an older generation is dropped
    /// whole: something changed after its read began, so it may list a
    /// result that is gone. A failed rows read keeps the last rows, marked as
    /// not refreshed. A failed count read leaves fresh rows fresh and only
    /// the count unknown.
    pub fn finish(&mut self, generation: u64, fetched: Option<Fetched>) -> bool {
        // Only one read is in flight at a time, so its reply ends the load
        // whether or not it applies.
        self.loading = false;
        if generation != self.generation {
            return false;
        }
        let Some(fetched) = fetched else {
            self.unavailable = true;
            self.active = None;
            return true;
        };
        match fetched.rows {
            Ok((mut rows, partial, sidebar_partial)) => {
                // A read started after a removal is authoritative for it.
                self.removed.retain(|(_, gone)| generation <= *gone);
                rows.retain(|row| !self.removed.iter().any(|(id, _)| *id == row.id));
                self.rows = rows;
                self.rows.truncate(SIDEBAR_LIMIT.into());
                self.partial = partial;
                self.sidebar_partial = sidebar_partial;
                self.unavailable = false;
            }
            Err(_) => self.unavailable = true,
        }
        self.active = fetched.active.ok();
        true
    }

    /// Starts and finishes a read at once, for tests that only need a list.
    #[cfg(test)]
    pub fn apply(&mut self, fetched: Option<Fetched>) {
        let generation = self.begin();
        self.finish(generation, fetched);
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

    /// Drops a result that is known to be gone, before the list is read
    /// again, so neither list offers a record that no longer exists. Any
    /// read already in flight is outdated, and until a read started after
    /// this one succeeds, no reply lists the result.
    pub fn forget(&mut self, id: &str) -> bool {
        self.removed.retain(|(gone, _)| gone != id);
        self.removed.push((id.to_owned(), self.generation));
        self.generation += 1;
        let before = self.rows.len();
        self.rows.retain(|row| row.id != id);
        self.rows.len() != before
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

/// The status as words, for the row's accessible name: the icon alone
/// tells a screen reader nothing.
pub(super) fn status_word(status: Status) -> &'static str {
    match status {
        Status::Pending => "pending",
        Status::Running => "running",
        Status::Completed => "completed",
        Status::Failed => "failed",
        Status::Unknown => "status unknown",
    }
}

/// The status icon's colour: Electron's hue, kept at 3:1 in both themes
/// (see [`super::palette`]). Electron draws running in the accent colour;
/// Buddy Dark's accent measured 2.6:1 on the sidebar, so running uses the
/// theme's `info` blue.
pub(super) fn status_color(status: Status, theme: &Theme) -> Hsla {
    use super::palette::{ORANGE, RED, TEAL, legible};
    match status {
        Status::Pending => legible(ORANGE, theme),
        Status::Running => theme.info,
        Status::Completed => legible(TEAL, theme),
        Status::Failed => legible(RED, theme),
        Status::Unknown => theme.muted_foreground,
    }
}

/// A sidebar row's accessible name: the whole label, the status and the
/// age, as in "PR Review: Fix the build, completed, 1 minute ago".
pub(super) fn accessible_name(row: &ResultSummary, now_ms: f64) -> String {
    format!(
        "{}, {}, {}",
        sidebar_label(row),
        status_word(row.status),
        age(row, now_ms)
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use buddy_core::copilot_results::summaries::parse_results;

    fn rows(ids: &[&str]) -> Vec<ResultSummary> {
        let page: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({"_id": id, "promptPreview": id, "status": "completed",
                    "categoryLabel": "general", "createdAt": 0.0, "hasOutput": false})
            })
            .collect();
        let wire = serde_json::json!({"page": page, "isDone": true, "continueCursor": ""});
        parse_results(&serde_json::to_vec(&wire).unwrap())
            .unwrap()
            .rows
    }

    fn ids(recent: &Recent) -> Vec<&str> {
        recent.rows.iter().map(|row| row.id.as_str()).collect()
    }

    #[test]
    fn a_removed_result_stays_out_until_a_read_started_after_it_succeeds() {
        let mut recent = Recent::default();
        recent.apply(Some(fetched(rows(&["kept", "gone"]), None)));
        let in_flight = recent.begin();
        recent.forget("gone");
        // Even a reply that slipped past the generation check, as if from
        // the same generation, does not list the removed result again.
        recent.generation = in_flight;
        assert!(recent.finish(in_flight, Some(fetched(rows(&["kept", "gone"]), None))));
        assert_eq!(ids(&recent), ["kept"]);
        // A failed read keeps the entry.
        recent.apply(Some(failed_rows(None)));
        assert_eq!(recent.removed.len(), 1);
        // The first successful read started after the removal settles it.
        recent.apply(Some(fetched(rows(&["new", "kept"]), None)));
        assert_eq!(ids(&recent), ["new", "kept"]);
        assert!(recent.removed.is_empty());
    }

    #[test]
    fn a_reply_from_an_older_read_is_dropped_whole() {
        let mut recent = Recent::default();
        recent.apply(Some(fetched(rows(&["kept"]), None)));
        let old = recent.begin();
        recent.forget("other");
        let counts = ActiveCounts {
            pending: 2,
            running: 0,
        };
        assert!(!recent.finish(old, Some(fetched(rows(&["older"]), Some(counts)))));
        assert!(!recent.loading);
        assert_eq!(ids(&recent), ["kept"]);
        assert_eq!(recent.badge(), None);
    }
}
