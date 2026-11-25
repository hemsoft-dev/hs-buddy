//! The organization overview's rate-limit gauge (Electron's `RateLimitGauge`)
//! and its "Current workbench" (Electron's `ActiveRepositoriesSection`).
use super::quota::{QuotaReply, SearchHold};
use super::*;
use buddy_core::github::{
    ActivityItem, GitHubError, RateLimit, RepositoryActivity, RepositorySummary, RequestTicket,
};
use jiff::Timestamp;

/// An optional overview section. It is read on its own, after or beside the
/// overview, so it can never delay or fail the repository list.
#[derive(Debug, Clone, Default)]
pub(super) enum Section<T> {
    /// Not part of this page, or not yet known to apply.
    #[default]
    Hidden,
    Loading,
    Failed,
    Ready(T),
}

impl<T> Section<T> {
    pub(super) fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// The overview's workbench for one account. Each read has a request number:
/// a reply is applied only while it is the newest read, for the same account,
/// on the overview. The quota gauge is kept per login, for longer than the
/// overview, in [`QuotaTracker`](super::quota::QuotaTracker).
#[derive(Default)]
pub(super) struct OverviewExtras {
    account: Option<AccountScope>,
    pub(super) workbench: Section<RepositoryActivity>,
    pub(super) workbench_request: u64,
    /// The namespace lookup finished while the GitHub section was hidden.
    /// Its searches start only when the section is shown again.
    deferred: Option<(Vec<RepositorySummary>, bool)>,
}

impl OverviewExtras {
    /// Forget the workbench; replies to earlier reads are dropped.
    pub(super) fn reset(&mut self, account: Option<AccountScope>) {
        *self = Self {
            account,
            workbench_request: self.workbench_request.wrapping_add(1),
            ..Self::default()
        };
    }
}

const HEALTHY_THRESHOLD: f32 = 0.6;
const WARNING_THRESHOLD: f32 = 0.3;

/// Electron's `getGaugeColor`: green while healthy, amber, then red.
pub(super) fn gauge_color(ratio: f32) -> u32 {
    let ratio = ratio.clamp(0.0, 1.0);
    let (r, g, b) = if ratio > HEALTHY_THRESHOLD {
        let t = (ratio - HEALTHY_THRESHOLD) / (1.0 - HEALTHY_THRESHOLD);
        (80.0 + (1.0 - t) * 140.0, 200.0 - (1.0 - t) * 30.0, 80.0)
    } else if ratio > WARNING_THRESHOLD {
        let t = (ratio - WARNING_THRESHOLD) / WARNING_THRESHOLD;
        (240.0 - t * 20.0, 120.0 + t * 50.0, 50.0)
    } else {
        let t = ratio / WARNING_THRESHOLD;
        (220.0 + (1.0 - t) * 20.0, t * 120.0, 40.0)
    };
    let channel = |value: f32| value.round() as u32;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// The gauge's value and caption: `4,988` and `/ 5,000 · resets 12m`.
pub(super) fn gauge_text(limit: &RateLimit, now_seconds: i64) -> (String, String) {
    (
        buddy_core::format::thousands(limit.remaining as i64),
        format!(
            "/ {} · resets {}",
            buddy_core::format::thousands(limit.limit as i64),
            limit.resets_in(now_seconds)
        ),
    )
}

fn ago(at: Timestamp, now: Timestamp) -> String {
    let elapsed = now
        .as_millisecond()
        .saturating_sub(at.as_millisecond())
        .max(0) as u64;
    buddy_core::format::distance_to_now(elapsed)
}

/// The header's repository total and freshness. An unread workbench has no
/// total: Electron's "0 repositories" would claim there was no activity.
pub(super) fn header_meta(activity: &RepositoryActivity, now: Timestamp) -> Vec<String> {
    let mut meta = Vec::new();
    // An empty list from an unread or unfinished search, or from a sample
    // GitHub says has more results, is not "0 repositories".
    if !activity.repositories.is_empty() || !(activity.uncertain() || activity.has_more) {
        let count = activity.repositories.len();
        meta.push(format!(
            "{count} {}",
            if count == 1 {
                "repository"
            } else {
                "repositories"
            }
        ));
    }
    meta.push(format!("Updated {}", ago(activity.fetched_at, now)));
    meta
}

/// The longest `Retry-After` GitHub gave either search, if one was limited.
fn search_throttle(activity: &RepositoryActivity) -> Option<u64> {
    [&activity.issues_error, &activity.pull_requests_error]
        .into_iter()
        .filter_map(|error| match error {
            Some(GitHubError::RateLimited {
                retry_after_seconds,
            }) => Some(*retry_after_seconds),
            _ => None,
        })
        .max()
}

/// The workbench shown while GitHub's search window is open and no search
/// is sent: a shown workbench that already says GitHub limited a search
/// stays; anything else becomes an unread workbench limited by GitHub, read
/// when GitHub throttled the login's searches.
pub(super) fn held_workbench(
    shown: Option<&RepositoryActivity>,
    hold: SearchHold,
    now: Timestamp,
) -> Option<RepositoryActivity> {
    if shown.and_then(search_throttle).is_some() {
        return None;
    }
    let limited = || {
        Some(GitHubError::RateLimited {
            retry_after_seconds: hold.wait_seconds(),
        })
    };
    Some(RepositoryActivity {
        repositories: Vec::new(),
        issues_error: limited(),
        pull_requests_error: limited(),
        issues_incomplete: false,
        pull_requests_incomplete: false,
        has_more: false,
        fetched_at: jiff::SignedDuration::try_from(hold.since)
            .ok()
            .and_then(|since| now.checked_sub(since).ok())
            .unwrap_or(now),
    })
}

/// What the section says above its cards, if anything. While GitHub's
/// search window is open, `wait_seconds` is the time left, and a limited
/// search reports that instead of the `Retry-After` it was given.
pub(super) fn status(activity: &RepositoryActivity, wait_seconds: Option<u64>) -> Option<String> {
    let reason = |error: &Option<GitHubError>| {
        error.as_ref().map(|error| match (error, wait_seconds) {
            (GitHubError::RateLimited { .. }, Some(retry_after_seconds)) => {
                GitHubError::RateLimited {
                    retry_after_seconds,
                }
                .to_string()
            }
            _ => error.to_string(),
        })
    };
    if activity.unavailable() {
        return Some(format!(
            "Repository activity is unavailable. The rest of the overview is still current. GitHub reported: {}",
            reason(&activity.issues_error).unwrap_or_default()
        ));
    }
    if activity.partial() {
        let (side, error) = match &activity.issues_error {
            Some(_) => ("Issues", &activity.issues_error),
            None => ("Pull requests", &activity.pull_requests_error),
        };
        return Some(format!(
            "Showing the activity GitHub returned. One side could not be refreshed. {side}: {}",
            reason(error).unwrap_or_default()
        ));
    }
    if activity.issues_incomplete || activity.pull_requests_incomplete {
        return Some(
            "GitHub's search timed out before it finished, so this activity may be incomplete."
                .into(),
        );
    }
    if !activity.repositories.is_empty() {
        return None;
    }
    // Everything sampled was filtered out, but GitHub reported more results.
    Some(if activity.has_more {
        "No activity in the first results GitHub returned; more may exist. All activity shows the rest on GitHub.".into()
    } else {
        "No recent issue or pull-request activity found.".into()
    })
}

/// A lane's heading count, and the note under its items: an unread lane
/// shows `!` and "Activity unavailable", never `0`, and one from a search
/// that timed out says it may be missing items.
pub(super) fn lane_summary(
    items: &[ActivityItem],
    error: &Option<GitHubError>,
    incomplete: bool,
) -> (String, Option<&'static str>) {
    match error {
        Some(_) => ("!".into(), Some("Activity unavailable")),
        None if incomplete && items.is_empty() => ("!".into(), Some("Search incomplete")),
        None if incomplete => (
            format!("{}+", items.len()),
            Some("Search incomplete; more may exist"),
        ),
        None if items.is_empty() => ("0".into(), Some("Nothing recent")),
        None => (items.len().to_string(), None),
    }
}

pub(super) fn item_meta(item: &ActivityItem, now: Timestamp) -> String {
    format!("{} · {}", item.state.label(), ago(item.updated_at, now))
}

impl GitHubView {
    pub(super) fn rate_limit_gauge(
        &self,
        limit: &RateLimit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let ratio = limit.ratio().clamp(0.0, 1.0);
        let color = gpui_kit::rgb(gauge_color(ratio));
        let (value, caption) = gauge_text(limit, Timestamp::now().as_second());
        v_flex()
            .id("github-rate-limit")
            .gap_1()
            .items_end()
            .child(
                h_flex()
                    .gap_1()
                    .items_baseline()
                    .child(
                        div()
                            .text_color(color)
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(value),
                    )
                    .child(div().text_color(cx.theme().muted_foreground).child(caption)),
            )
            .child(
                div()
                    .w(zpx(160.0))
                    .h(zpx(5.0))
                    .rounded(zpx(3.0))
                    .bg(cx.theme().secondary)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(ratio))
                            .rounded(zpx(3.0))
                            .bg(color),
                    ),
            )
    }

    pub(super) fn workbench(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let (activity, placeholder) = match &self.overview.workbench {
            Section::Hidden => return None,
            Section::Loading => (None, Some("Loading recent activity...")),
            Section::Failed => (
                None,
                Some(
                    "Repository activity is unavailable. The rest of the overview is still current.",
                ),
            ),
            Section::Ready(activity) => (Some(activity), None),
        };
        let now = Timestamp::now();
        let wait = self.search_hold().map(|hold| hold.wait_seconds());
        let muted = cx.theme().muted_foreground;
        let mut header = h_flex().flex_wrap().gap_2().items_center();
        for meta in activity.map(|a| header_meta(a, now)).unwrap_or_default() {
            header = header.child(div().text_color(muted).child(meta));
        }
        header = header.child(
            Button::new("github-workbench-refresh")
                .label("Refresh")
                .disabled(self.loading || matches!(self.overview.workbench, Section::Loading))
                .on_click(cx.listener(|this, _, _, cx| this.load(true, false, cx))),
        );
        if activity.is_some_and(|a| a.has_more)
            && let Some(account) = &self.account
        {
            let url = github::all_activity_url(account.organization());
            header = header.child(
                Button::new("github-workbench-all")
                    .label("All activity")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        let mut section = v_flex()
            .id("github-workbench")
            .gap_3()
            .p_3()
            .border_1()
            .rounded(zpx(8.0))
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .child(div().text_color(muted).child("Current workbench"))
                            .child(
                                div()
                                    .text_size(zpx(16.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child("Active repositories"),
                            ),
                    )
                    .child(header),
            );
        if let Some(text) = placeholder
            .map(String::from)
            .or_else(|| activity.and_then(|activity| status(activity, wait)))
        {
            section = section.child(div().whitespace_normal().child(text));
        }
        let Some(activity) = activity else {
            return Some(section.into_any_element());
        };
        for (index, repository) in activity.repositories.iter().enumerate() {
            let rank = index + 1;
            let url = repository.repository.web_url();
            let mut lanes = h_flex().flex_wrap().gap_3().items_start();
            for (kind, title, items, error, incomplete) in [
                (
                    "issues",
                    "Issues",
                    &repository.issues,
                    &activity.issues_error,
                    activity.issues_incomplete,
                ),
                (
                    "pulls",
                    "Pull requests",
                    &repository.pull_requests,
                    &activity.pull_requests_error,
                    activity.pull_requests_incomplete,
                ),
            ] {
                let (count, note) = lane_summary(items, error, incomplete);
                let mut lane = v_flex().flex_1().min_w(zpx(240.0)).gap_1().child(
                    h_flex()
                        .justify_between()
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_color(muted).child(count)),
                );
                for item in items {
                    let url = item.url.clone();
                    lane = lane.child(
                        v_flex()
                            .child(
                                Button::new(format!(
                                    "github-workbench-{rank}-{kind}-{}",
                                    item.number
                                ))
                                .label(format!("#{} {}", item.number, item.title))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                            .child(div().text_color(muted).child(item_meta(item, now))),
                    );
                }
                if let Some(note) = note {
                    lane = lane.child(div().text_color(muted).child(note));
                }
                lanes = lanes.child(lane);
            }
            section = section.child(
                v_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_color(muted).child(format!("{rank:02}")))
                            .child(
                                Button::new(format!("github-workbench-repo-{rank}"))
                                    .label(repository.name().to_string())
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(ago(repository.updated_at, now)),
                            ),
                    )
                    .child(lanes),
            );
        }
        Some(section.into_any_element())
    }

    /// Start the overview's own reads: forget another account's sections,
    /// then read the quota beside the repository list.
    pub(super) fn begin_overview(
        &mut self,
        ticket: &RequestTicket,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.overview.account.as_ref() != Some(&ticket.account) {
            self.overview.reset(Some(ticket.account.clone()));
        }
        self.read_rate_limit(ticket.account.clone(), force, cx);
    }

    fn accepts_overview(&self, account: &AccountScope) -> bool {
        self.route == Route::Organization
            && self.overview.account.as_ref() == Some(account)
            && self.account.as_ref() == Some(account)
    }

    /// The quota the gauge shows: the selected login's, on the overview.
    pub(super) fn shown_rate_limit(&self) -> Option<&RateLimit> {
        if self.route != Route::Organization {
            return None;
        }
        self.quota.shown(self.account.as_ref()?)
    }

    /// Re-read the quota on Electron's 60-second interval while the overview
    /// is shown, and not before a throttled read's retry window.
    pub(super) fn poll_rate_limit(&mut self, cx: &mut Context<Self>) {
        if self.route != Route::Organization || self.data.is_none() {
            return;
        }
        let Some(account) = self.overview.account.clone() else {
            return;
        };
        self.read_rate_limit(account, false, cx);
    }

    /// Read the quota if the login may send now. Unless forced, the login's
    /// reading is reused while it is under a minute old or a read is in
    /// flight, in whichever namespace it was read. The reply is recorded for
    /// the login even if the overview was left meanwhile.
    fn read_rate_limit(&mut self, account: AccountScope, force: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        if !force && !self.quota.due(&account, now) {
            return;
        }
        let Some(request) = self.quota.begin(&account, now) else {
            return;
        };
        let client = self.clients.for_account(account);
        let rx = Runtime::global(cx).spawn(async move { client.rate_limit().await });
        cx.spawn(async move |this, cx| {
            let reply = match rx.await {
                Ok(Ok(loaded)) => QuotaReply::Read(loaded.value, loaded.age),
                Ok(Err(GitHubError::RateLimited {
                    retry_after_seconds,
                })) => QuotaReply::Throttled {
                    retry_after_seconds,
                },
                Ok(Err(_)) | Err(_) => QuotaReply::Failed,
            };
            this.update(cx, |this, cx| {
                let now_seconds = Timestamp::now().as_second();
                if this
                    .quota
                    .finish(&request, reply, Instant::now(), now_seconds)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Start a workbench whose namespace lookup finished while the GitHub
    /// section was hidden, unless a reload is already reading it again.
    pub(super) fn resume_workbench(&mut self, cx: &mut Context<Self>) {
        let Some((repositories, force)) = self.overview.deferred.take() else {
            return;
        };
        let current = self
            .overview
            .account
            .as_ref()
            .is_some_and(|account| self.accepts_overview(account));
        if self.active && !self.loading && current {
            self.load_workbench(repositories, force, cx);
        }
    }

    /// Rank recent activity once the repository list is shown. Electron does
    /// this for user namespaces only; an unread namespace shows no workbench.
    /// A shown workbench stays until its refresh arrives.
    pub(super) fn load_workbench(
        &mut self,
        repositories: Vec<RepositorySummary>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.overview.account.clone() else {
            return;
        };
        self.overview.deferred = None;
        self.overview.workbench_request = self.overview.workbench_request.wrapping_add(1);
        let request = self.overview.workbench_request;
        let client = self.clients.for_account(account.clone());
        let rx = Runtime::global(cx).spawn(async move {
            if !client.namespace_is_personal(false).await.unwrap_or(false) {
                return None;
            }
            Some(client)
        });
        cx.spawn(async move |this, cx| {
            let personal = rx.await;
            this.update(cx, |this, cx| {
                if !this.accepts_workbench(&account, request) {
                    return;
                }
                match personal {
                    // No search starts for a hidden GitHub section.
                    Ok(Some(_)) if !this.active => {
                        this.overview.deferred = Some((repositories, force));
                    }
                    Ok(Some(client)) => {
                        this.read_activity(client, request, repositories, force, cx)
                    }
                    _ => this.overview.workbench = Section::Hidden,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// GitHub's search window for the overview's login, while it is open.
    pub(super) fn search_hold(&self) -> Option<SearchHold> {
        self.quota
            .search_hold(self.overview.account.as_ref()?, Instant::now())
    }

    fn accepts_workbench(&self, account: &AccountScope, request: u64) -> bool {
        self.overview.workbench_request == request && self.accepts_overview(account)
    }

    fn read_activity(
        &mut self,
        client: github::GitHubClient,
        request: u64,
        repositories: Vec<RepositorySummary>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let account = client.account.clone();
        // While GitHub's search window for the login is open, neither
        // Refresh nor auto-refresh searches; the workbench keeps saying why.
        let Some(search) = self.quota.begin_search(&account, Instant::now()) else {
            if let Some(held) = self.search_hold().and_then(|hold| {
                held_workbench(self.overview.workbench.ready(), hold, Timestamp::now())
            }) {
                self.overview.workbench = Section::Ready(held);
            }
            return;
        };
        if self.overview.workbench.ready().is_none() {
            self.overview.workbench = Section::Loading;
        }
        let rx = Runtime::global(cx)
            .spawn(async move { client.repository_activity(&repositories, force).await });
        cx.spawn(async move |this, cx| {
            let activity = rx.await;
            this.update(cx, |this, cx| {
                // GitHub's window applies to the credential, so it is kept
                // even when the overview has moved on.
                if let Ok(activity) = &activity {
                    this.quota
                        .finish_search(&search, search_throttle(activity), Instant::now());
                }
                if !this.accepts_workbench(&account, request) {
                    return;
                }
                match activity {
                    Ok(activity) => this.overview.workbench = Section::Ready(activity),
                    // Electron keeps the shown activity when a refresh fails.
                    Err(_) if this.overview.workbench.ready().is_some() => {}
                    Err(_) => this.overview.workbench = Section::Failed,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
