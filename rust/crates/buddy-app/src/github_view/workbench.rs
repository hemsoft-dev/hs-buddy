//! The organization overview's rate-limit gauge (Electron's `RateLimitGauge`)
//! and its "Current workbench" (Electron's `ActiveRepositoriesSection`).
use super::*;
use buddy_core::github::{ActivityItem, GitHubError, RateLimit, RepositoryActivity};
use jiff::Timestamp;

/// Electron's gauge re-reads the quota every 60 seconds.
pub(super) const RATE_LIMIT_REFRESH: Duration = Duration::from_secs(60);
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
    if !activity.unavailable() {
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
    activity
        .repositories
        .is_empty()
        .then(|| "No recent issue or pull-request activity found.".into())
}

/// A lane's heading count, and its text when it lists nothing: an unread
/// lane shows `!` and "Activity unavailable", never `0`.
pub(super) fn lane_summary(
    items: &[ActivityItem],
    error: &Option<GitHubError>,
) -> (String, Option<&'static str>) {
    match error {
        Some(_) => ("!".into(), Some("Activity unavailable")),
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

    pub(super) fn workbench(
        &self,
        activity: &RepositoryActivity,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let now = Timestamp::now();
        let muted = cx.theme().muted_foreground;
        let mut header = h_flex().flex_wrap().gap_2().items_center();
        for meta in header_meta(activity, now) {
            header = header.child(div().text_color(muted).child(meta));
        }
        header = header.child(
            Button::new("github-workbench-refresh")
                .label("Refresh")
                .disabled(self.loading)
                .on_click(cx.listener(|this, _, _, cx| this.load(true, false, cx))),
        );
        if activity.has_more
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
        if let Some(status) = status(activity) {
            section = section.child(div().whitespace_normal().child(status));
        }
        for (index, repository) in activity.repositories.iter().enumerate() {
            let rank = index + 1;
            let url = repository.repository.web_url();
            let mut lanes = h_flex().flex_wrap().gap_3().items_start();
            for (kind, title, items, error) in [
                (
                    "issues",
                    "Issues",
                    &repository.issues,
                    &activity.issues_error,
                ),
                (
                    "pulls",
                    "Pull requests",
                    &repository.pull_requests,
                    &activity.pull_requests_error,
                ),
            ] {
                let (count, empty) = lane_summary(items, error);
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
                if let Some(empty) = empty {
                    lane = lane.child(div().text_color(muted).child(empty));
                } else {
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
        section
    }

    /// Re-read the quota on Electron's 60-second interval while the overview
    /// is shown. A result for another account or page is dropped, and a
    /// failed read hides the gauge rather than showing an empty quota.
    pub(super) fn poll_rate_limit(&mut self, cx: &mut Context<Self>) {
        if self.route != Route::Organization
            || self.rate_limit_loading
            || self.data.is_none()
            || self
                .rate_limit_read_at
                .is_some_and(|at| at.elapsed() < RATE_LIMIT_REFRESH)
        {
            return;
        }
        let Some(ticket) = self.selection.ticket() else {
            return;
        };
        self.rate_limit_read_at = Some(Instant::now());
        self.rate_limit_loading = true;
        let client = self.clients.for_account(ticket.account.clone());
        let rx = Runtime::global(cx).spawn(async move { client.rate_limit(true).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.rate_limit_loading = false;
                if let Ok(result) = result
                    && this.selection.accepts(&ticket)
                    && this.route == Route::Organization
                    && let Some(data) = &mut this.data
                {
                    data.rate_limit = result.ok();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
