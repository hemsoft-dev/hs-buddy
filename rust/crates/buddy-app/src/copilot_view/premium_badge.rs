//! Electron's `PremiumUsageBadge`: the chosen account's premium usage beside
//! the account field. Each account's quota is kept for five minutes and shared
//! by every view, as Electron's module-level cache is. Nothing here writes.
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use buddy_core::config::GitHubAccount;
use buddy_core::copilot_usage::UsagePool;
use futures::future::BoxFuture;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, div, rgb,
};

use crate::zoom::zpx;

/// `CACHE_TTL` in `PremiumUsageBadge.tsx`.
pub(super) const TTL: Duration = Duration::from_secs(5 * 60);

/// The `premium_interactions` figures the badge shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Quota {
    pub remaining: i64,
    pub entitlement: i64,
    pub percent_used: f64,
}

impl Quota {
    pub fn from_pool(pool: &UsagePool) -> Self {
        // GitHub's percent_remaining is remaining over entitlement, and 100
        // for an unlimited (zero) entitlement.
        let percent_used = if pool.allotment > 0 {
            pool.used as f64 / pool.allotment as f64 * 100.0
        } else {
            0.0
        };
        Self {
            remaining: pool.allotment - pool.used,
            entitlement: pool.allotment,
            percent_used,
        }
    }

    /// `pct.toFixed(0)%`, then the remaining and included counts that
    /// Electron's tooltip names.
    pub fn text(&self) -> String {
        format!(
            "{}% · {}/{}",
            js_fixed(self.percent_used, 0),
            self.remaining,
            self.entitlement
        )
    }

    /// Electron's `title`.
    pub fn tooltip(&self) -> String {
        format!(
            "Premium: {} of {} remaining ({}% used)",
            self.remaining,
            self.entitlement,
            js_fixed(self.percent_used, 1)
        )
    }

    /// `getQuotaColor(pct)` in `quotaUtils.ts`.
    pub fn color(&self) -> u32 {
        let pct = self.percent_used;
        if pct >= 90.0 {
            0xe85d5d
        } else if pct >= 75.0 {
            0xe89b3c
        } else if pct >= 50.0 {
            0xdcd34a
        } else {
            0x4ec9b0
        }
    }
}

/// `Number.prototype.toFixed`, which rounds a half away from zero where
/// Rust's formatting rounds it to even.
fn js_fixed(value: f64, digits: i32) -> String {
    let scale = 10f64.powi(digits);
    let rounded = (value * scale).round() / scale;
    format!("{rounded:.*}", digits as usize)
}

/// Electron's module-level `quotaCache`, keyed by the account's login.
#[derive(Default)]
pub(super) struct QuotaCache(HashMap<String, (Quota, Instant)>);
impl gpui_kit::Global for QuotaCache {}

impl QuotaCache {
    pub fn fresh(&self, login: &str, now: Instant) -> Option<Quota> {
        self.0
            .get(&login.to_ascii_lowercase())
            .filter(|(_, at)| now.saturating_duration_since(*at) < TTL)
            .map(|(quota, _)| *quota)
    }

    pub fn store(&mut self, login: &str, quota: Quota, now: Instant) {
        self.0.insert(login.to_ascii_lowercase(), (quota, now));
    }
}

/// What the badge shows. Electron renders nothing while loading, after a
/// failure or without an account, so none of those ever shows a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum State {
    Hidden,
    Loading,
    Failed,
    Shown(Quota),
}

pub(super) type Fetch = Rc<dyn Fn(GitHubAccount) -> BoxFuture<'static, Result<UsagePool, String>>>;

/// The production read: the same `/copilot_internal/user` quota the Usage
/// pane reads for a personal account, with the account's own credential.
pub(super) fn fetch() -> Fetch {
    Rc::new(|account| {
        Box::pin(async move {
            buddy_core::copilot_usage::fetch_personal_quota(&account, jiff::Timestamp::now()).await
        })
    })
}

pub(super) struct Badge {
    /// The configured login the badge is for, or empty for none.
    pub login: String,
    pub state: State,
    /// Bumped for each read, so an earlier account's late answer is dropped.
    pub generation: u64,
    pub fetch: Fetch,
}

impl Default for Badge {
    fn default() -> Self {
        Self {
            login: String::new(),
            state: State::Hidden,
            generation: 0,
            fetch: fetch(),
        }
    }
}

/// The configured account a typed login names. Electron's picker offers only
/// configured accounts; a login that is not one has no quota to show.
pub(super) fn configured(typed: &str, cx: &App) -> Option<GitHubAccount> {
    let typed = typed.trim();
    if typed.is_empty() || !cx.has_global::<crate::settings::Settings>() {
        return None;
    }
    crate::settings::Settings::global(cx)
        .config
        .github
        .accounts
        .iter()
        .find(|account| account.username.eq_ignore_ascii_case(typed))
        .cloned()
}

/// The badge: a zap icon, a bar filled to the share used and its figures.
pub(super) fn render(quota: Quota, cx: &App) -> impl IntoElement {
    let color = rgb(quota.color());
    let track = cx.theme().muted;
    h_flex()
        .id("copilot-premium-usage")
        .gap(zpx(5.0))
        .items_center()
        .px(zpx(6.0))
        .py(zpx(2.0))
        .rounded(zpx(10.0))
        .bg(cx.theme().secondary)
        .tooltip({
            let tooltip = quota.tooltip();
            move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            }
        })
        .child(Icon::new(IconName::Zap).xsmall().text_color(color))
        .child(
            div()
                .w(zpx(48.0))
                .h(zpx(4.0))
                .rounded(zpx(2.0))
                .bg(track)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(zpx(
                            48.0 * (quota.percent_used.clamp(0.0, 100.0) as f32) / 100.0
                        ))
                        .bg(color),
                ),
        )
        .child(
            div()
                .text_size(zpx(11.0))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(color)
                .child(quota.text()),
        )
}
