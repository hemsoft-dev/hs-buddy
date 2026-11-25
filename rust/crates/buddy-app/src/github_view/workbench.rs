//! The organization overview's rate-limit gauge (Electron's `RateLimitGauge`)
//! and its "Current workbench" (Electron's `ActiveRepositoriesSection`).
use super::*;
use buddy_core::github::{
    ActivityItem, GitHubError, RateLimit, RepositoryActivity, RepositorySummary, RequestTicket,
};
use jiff::Timestamp;

/// Electron's gauge re-reads the quota every 60 seconds.
pub(super) const RATE_LIMIT_REFRESH: Duration = Duration::from_secs(60);

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

/// The overview's quota gauge and workbench for one account. Each read has a
/// request number: a reply is applied only while it is the newest read, for
/// the same account, on the overview.
#[derive(Default)]
pub(super) struct OverviewExtras {
    account: Option<AccountScope>,
    pub(super) rate_limit: Section<RateLimit>,
    /// When GitHub reported the shown quota, or when the last read failed.
    pub(super) rate_limit_read_at: Option<Instant>,
    /// GitHub asked this account for no quota read before then. It outlives
    /// leaving the overview, so reopening it does not read sooner.
    pub(super) rate_limit_retry: Option<(AccountScope, Instant)>,
    pub(super) rate_limit_loading: bool,
    rate_limit_request: u64,
    pub(super) workbench: Section<RepositoryActivity>,
    pub(super) workbench_request: u64,
}

impl OverviewExtras {
    /// Forget the sections; replies to earlier reads are dropped.
    pub(super) fn reset(&mut self, account: Option<AccountScope>) {
        *self = Self {
            account,
            rate_limit_retry: self.rate_limit_retry.take(),
            rate_limit_request: self.rate_limit_request.wrapping_add(1),
            workbench_request: self.workbench_request.wrapping_add(1),
            ..Self::default()
        };
    }
}

/// How long a throttled quota read must wait: GitHub's retry window, or the
/// reset of a quota last seen exhausted if later, and never less than
/// Electron's minute. Other failures retry on the ordinary minute.
pub(super) fn quota_retry_delay(
    error: &GitHubError,
    last: Option<&RateLimit>,
    now_seconds: i64,
) -> Option<Duration> {
    let GitHubError::RateLimited {
        retry_after_seconds,
    } = error
    else {
        return None;
    };
    let mut seconds = RATE_LIMIT_REFRESH.as_secs().max(*retry_after_seconds);
    if let Some(last) = last.filter(|last| last.remaining == 0) {
        seconds = seconds.max(last.reset.saturating_sub(now_seconds).max(0) as u64);
    }
    Some(Duration::from_secs(seconds))
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
    // An empty list from an unread or unfinished search is not "0 repositories".
    if !activity.repositories.is_empty() || !activity.uncertain() {
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

/// What the section says above its cards, if anything.
pub(super) fn status(activity: &RepositoryActivity) -> Option<String> {
    let reason = |error: &Option<GitHubError>| error.as_ref().map(ToString::to_string);
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
    activity
        .repositories
        .is_empty()
        .then(|| "No recent issue or pull-request activity found.".into())
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
            .or_else(|| activity.and_then(status))
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

    /// Re-read the quota on Electron's 60-second interval while the overview
    /// is shown, and not before a throttled read's retry window.
    pub(super) fn poll_rate_limit(&mut self, cx: &mut Context<Self>) {
        if self.route != Route::Organization
            || self.overview.rate_limit_loading
            || self.data.is_none()
            || self
                .overview
                .rate_limit_read_at
                .is_some_and(|at| at.elapsed() < RATE_LIMIT_REFRESH)
        {
            return;
        }
        let Some(account) = self.overview.account.clone() else {
            return;
        };
        self.read_rate_limit(account, true, cx);
    }

    /// A failed read hides the gauge rather than showing an empty quota; a
    /// cached reading keeps the time GitHub reported it.
    fn read_rate_limit(&mut self, account: AccountScope, force: bool, cx: &mut Context<Self>) {
        if self
            .overview
            .rate_limit_retry
            .as_ref()
            .is_some_and(|(throttled, at)| *throttled == account && Instant::now() < *at)
        {
            return;
        }
        self.overview.rate_limit_request = self.overview.rate_limit_request.wrapping_add(1);
        let request = self.overview.rate_limit_request;
        self.overview.rate_limit_loading = true;
        if self.overview.rate_limit.ready().is_none() {
            self.overview.rate_limit = Section::Loading;
        }
        let client = self.clients.for_account(account.clone());
        let rx = Runtime::global(cx).spawn(async move { client.rate_limit(force).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                if this.overview.rate_limit_request != request {
                    return;
                }
                this.overview.rate_limit_loading = false;
                if !this.accepts_overview(&account) {
                    return;
                }
                let now = Instant::now();
                match result {
                    Ok(Ok(loaded)) => {
                        this.overview.rate_limit = Section::Ready(loaded.value);
                        this.overview.rate_limit_read_at =
                            Some(now.checked_sub(loaded.age).unwrap_or(now));
                        this.overview.rate_limit_retry = None;
                    }
                    Ok(Err(error)) => {
                        let delay = quota_retry_delay(
                            &error,
                            this.overview.rate_limit.ready(),
                            Timestamp::now().as_second(),
                        );
                        this.overview.rate_limit = Section::Failed;
                        this.overview.rate_limit_read_at = Some(now);
                        this.overview.rate_limit_retry =
                            delay.map(|delay| (account.clone(), now + delay));
                    }
                    Err(_) => {
                        this.overview.rate_limit = Section::Failed;
                        this.overview.rate_limit_read_at = Some(now);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        if self.overview.workbench.ready().is_none() {
            self.overview.workbench = Section::Loading;
        }
        let account = client.account.clone();
        let rx = Runtime::global(cx)
            .spawn(async move { client.repository_activity(&repositories, force).await });
        cx.spawn(async move |this, cx| {
            let activity = rx.await;
            this.update(cx, |this, cx| {
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
