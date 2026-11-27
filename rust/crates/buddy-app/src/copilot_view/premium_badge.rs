//! Electron's `PremiumUsageBadge`: the chosen account's premium usage beside
//! the account field. Each account's quota is kept for five minutes and shared
//! by every view, as Electron's module-level cache is. Nothing here writes.
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use buddy_core::config::GitHubAccount;
use buddy_core::copilot_usage::PremiumQuota;
use futures::future::BoxFuture;
use gpui_kit::assets::IconName;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::{
    App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, rgb,
};

use crate::zoom::zpx;

/// `CACHE_TTL` in `PremiumUsageBadge.tsx`.
pub(super) const TTL: Duration = Duration::from_secs(5 * 60);

/// The `premium_interactions` figures the badge shows, unrounded as
/// Electron keeps them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Quota {
    pub remaining: f64,
    pub entitlement: f64,
    pub percent_used: f64,
}

impl Quota {
    /// Electron's `percentUsed: 100 - p.percent_remaining`, with GitHub's
    /// own counts.
    pub fn from_premium(premium: &PremiumQuota) -> Self {
        Self {
            remaining: premium.remaining,
            entitlement: premium.entitlement,
            percent_used: 100.0 - premium.percent_remaining,
        }
    }

    /// `pct.toFixed(0)%`, then the remaining and included counts that
    /// Electron's tooltip names.
    pub fn text(&self) -> String {
        format!(
            "{}% · {}/{}",
            js_fixed(self.percent_used, 0),
            js_number(self.remaining),
            js_number(self.entitlement)
        )
    }

    /// Electron's `title`.
    pub fn tooltip(&self) -> String {
        format!(
            "Premium: {} of {} remaining ({}% used)",
            js_number(self.remaining),
            js_number(self.entitlement),
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

/// `Number.prototype.toFixed` for the magnitudes a quota reaches (below
/// 1e21). It rounds the binary value's exact decimal expansion, as Rust's
/// formatting does, but takes the larger magnitude on an exact half where
/// Rust takes the even digit: `(2.5).toFixed(0)` is `3`. Scaling first, as
/// `(x * 10).round()` would, rounds a value just under a half (`89.55` is
/// `89.5499…` in binary) up to `89.6`; JS shows `89.5`.
pub(super) fn js_fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    // Every f64 is a dyadic fraction, so 1100 places hold its exact
    // expansion (at most 1074 fractional digits).
    let exact = format!("{:.1100}", value.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(digits)).collect();
    if fraction
        .as_bytes()
        .get(digits)
        .is_some_and(|digit| *digit >= b'5')
    {
        let mut carry = true;
        for digit in kept.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            kept.insert(0, b'1');
        }
    }
    let point = kept.len() - digits;
    let mut text = String::from(if value < 0.0 { "-" } else { "" });
    text.push_str(std::str::from_utf8(&kept[..point]).expect("ASCII digits"));
    if digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&kept[point..]).expect("ASCII digits"));
    }
    text
}

/// A number interpolated into a JS template string: the shortest text that
/// reads back as the same value, as Rust's `Display` gives, with `-0` as `0`.
/// Quota counts never reach the 1e21 where JS switches to exponents.
fn js_number(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        format!("{value}")
    }
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

pub(super) type Fetch =
    Rc<dyn Fn(GitHubAccount) -> BoxFuture<'static, Result<PremiumQuota, String>>>;

/// The production read: the same `/copilot_internal/user` quota the Usage
/// pane reads for a personal account, with the account's own credential. An
/// incomplete response fails like any other read.
pub(super) fn fetch() -> Fetch {
    Rc::new(|account| {
        Box::pin(async move { buddy_core::copilot_usage::fetch_personal_quota(&account).await })
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

/// The badge's colours. Electron draws the figures in the quota colour too,
/// but on Buddy Light's `secondary` that measured 1.41–3.07:1 for 11 px
/// text, so the figures use `secondary.foreground`, which keeps 4.5:1 in
/// both themes (`muted.foreground` measured 4.2:1 in Buddy Dark). The quota colour stays on the icon and the bar, where 3:1 for
/// non-text applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Colors {
    pub background: Hsla,
    pub track: Hsla,
    pub tier: Hsla,
    pub figures: Hsla,
}

pub(super) fn colors(quota: &Quota, theme: &Theme) -> Colors {
    Colors {
        background: theme.secondary,
        track: theme.muted,
        tier: rgb(quota.color()).into(),
        figures: theme.secondary_foreground,
    }
}

/// The badge: a zap icon, a bar filled to the share used and its figures.
pub(super) fn render(quota: Quota, cx: &App) -> impl IntoElement {
    let colors = colors(&quota, cx.theme());
    h_flex()
        .id("copilot-premium-usage")
        .gap(zpx(5.0))
        .items_center()
        .px(zpx(6.0))
        .py(zpx(2.0))
        .rounded(zpx(10.0))
        .bg(colors.background)
        .tooltip({
            let tooltip = quota.tooltip();
            move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            }
        })
        .child(Icon::new(IconName::Zap).xsmall().text_color(colors.tier))
        .child(
            div()
                .w(zpx(48.0))
                .h(zpx(4.0))
                .rounded(zpx(2.0))
                .bg(colors.track)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(zpx(
                            48.0 * (quota.percent_used.clamp(0.0, 100.0) as f32) / 100.0
                        ))
                        .bg(colors.tier),
                ),
        )
        .child(
            div()
                .text_size(zpx(11.0))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(colors.figures)
                .child(quota.text()),
        )
}
