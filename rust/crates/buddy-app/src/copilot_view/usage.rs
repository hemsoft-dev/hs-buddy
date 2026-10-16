use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::zoom::zpx;
#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
use buddy_core::codex_usage::{self, UsageSnapshot};
use buddy_core::config::{GitHubAccount, GitHubConfig, UsageProvider};
use buddy_core::copilot_usage::history::{History, Page};
use buddy_core::copilot_usage::{self, BillingSnapshot, UsagePool};
use chrono::{Datelike, Utc};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

type RefreshReply = (
    Option<Result<UsageSnapshot, codex_usage::FetchError>>,
    Option<Result<UsagePool, String>>,
    Option<Result<BillingSnapshot, String>>,
    Option<Result<(), String>>,
);

pub(super) struct UsageView {
    enterprise: Entity<super::enterprise::EnterpriseView>,
    config: GitHubConfig,
    selected: usize,
    pool: Option<UsagePool>,
    billing: Option<BillingSnapshot>,
    codex: Option<UsageSnapshot>,
    history: Option<Page>,
    history_period: Option<(i32, u32)>,
    year: Entity<InputState>,
    month: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    history_error: Option<String>,
    notice: Option<String>,
}
impl UsageView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let now = Utc::now();
        let year = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(now.year().to_string())
                .placeholder("Year")
        });
        let month = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(now.month().to_string())
                .placeholder("Month")
        });
        for input in [&year, &month] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        Self {
            enterprise: cx.new(|cx| super::enterprise::EnterpriseView::new(window, cx)),
            config: GitHubConfig::default(),
            selected: 0,
            pool: None,
            billing: None,
            codex: None,
            history: None,
            history_period: None,
            year,
            month,
            busy: false,
            error: None,
            history_error: None,
            notice: None,
        }
    }
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let config = Settings::global(cx).config.github.clone();
        if config != self.config {
            self.config = config;
            self.selected = 0;
            self.clear();
        }
    }
    fn clear(&mut self) {
        self.pool = None;
        self.billing = None;
        self.codex = None;
        self.history = None;
        self.history_period = None;
        self.error = None;
        self.history_error = None;
        self.notice = None;
    }
    fn account(&self) -> Option<GitHubAccount> {
        self.config.accounts.get(self.selected).cloned()
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(account) = self.account() else {
            return;
        };
        let codex = self.config.effective_usage_provider(&account) == Some(UsageProvider::Codex);
        if codex
            && self.config.accounts.iter().position(|account| {
                self.config.effective_usage_provider(account) == Some(UsageProvider::Codex)
            }) != Some(self.selected)
        {
            self.error=Some("The local Codex login is assigned to the first configured Codex usage account. Choose Copilot for this account in Settings.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.pool = None;
        self.billing = None;
        self.codex = None;
        self.error = None;
        self.notice = None;
        let backend = History::new(&buddy_core::convex_data::convex_url());
        let enterprise = Settings::global(cx).config.ui.enterprise_slug.clone();
        let rx = Runtime::global(cx).spawn(async move {
            if codex {
                return (Some(codex_usage::fetch().await), None, None, None);
            }
            let now = Utc::now();
            let (pool, billing) = futures::join!(
                copilot_usage::fetch_account_pool(&account, now),
                copilot_usage::fetch_billing(&account, now, Some(&enterprise))
            );
            let save = if let Ok(billing) = &billing {
                Some(match backend {
                    Ok(backend) => backend.store(&account, billing).await,
                    Err(error) => Err(error),
                })
            } else {
                None
            };
            (None, Some(pool), Some(billing), save)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.finish_refresh(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn finish_refresh(
        &mut self,
        result: Result<RefreshReply, futures::channel::oneshot::Canceled>,
    ) {
        self.busy = false;
        match result {
            Ok((codex, pool, billing, save)) => {
                let mut errors = Vec::new();
                if let Some(result) = codex {
                    self.codex = match result {
                        Ok(data) => Some(data),
                        Err(error) => {
                            errors.push(error.to_string());
                            None
                        }
                    };
                }
                if let Some(result) = pool {
                    self.pool = match result {
                        Ok(data) => Some(data),
                        Err(_) => {
                            errors.push("Copilot allowance is unavailable for this account and organization.".into());
                            None
                        }
                    };
                }
                if let Some(result) = billing {
                    self.billing = match result {
                        Ok(data) => Some(data),
                        Err(error) => {
                            errors.push(error);
                            None
                        }
                    };
                }
                if let Some(result) = save {
                    self.notice = Some(match result {
                        Ok(()) => "Billing snapshot saved to history.".into(),
                        Err(error) => error,
                    });
                }
                self.error = (!errors.is_empty()).then(|| errors.join(" "));
            }
            Err(_) => {
                self.pool = None;
                self.billing = None;
                self.codex = None;
                self.error = Some("The usage request stopped. Try refreshing.".into());
            }
        }
    }
    fn load_history(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(account) = self.account() else {
            return;
        };
        let year = self.year.read(cx).value().trim().parse::<i32>();
        let month = self.month.read(cx).value().trim().parse::<u32>();
        let (Ok(year), Ok(month)) = (year, month) else {
            self.history_error = Some("Enter a numeric billing year and month.".into());
            cx.notify();
            return;
        };
        if older && self.history_period != Some((year, month)) {
            self.history_error =
                Some("Load the selected period before requesting older snapshots.".into());
            cx.notify();
            return;
        }
        let backend = match History::new(&buddy_core::convex_data::convex_url()) {
            Ok(backend) => backend,
            Err(error) => {
                self.history_error = Some(error);
                cx.notify();
                return;
            }
        };
        let cursor = if older {
            self.history
                .as_ref()
                .map(|page| page.continue_cursor.clone())
        } else {
            None
        };
        self.busy = true;
        self.history_error = None;
        let rx = Runtime::global(cx)
            .spawn(async move { backend.page(&account, year, month, cursor.as_deref()).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(page)) => {
                        this.history = Some(page);
                        this.history_period = Some((year, month));
                    }
                    Ok(Err(error)) => this.history_error = Some(error),
                    Err(_) => {
                        this.history_error =
                            Some("Usage history is unavailable. Try refreshing.".into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let mut accounts = h_flex().flex_wrap().gap_2();
        for (index, account) in self.config.accounts.iter().enumerate() {
            let provider =
                if self.config.effective_usage_provider(account) == Some(UsageProvider::Codex) {
                    "Codex"
                } else {
                    "Copilot"
                };
            accounts = accounts.child(
                Button::new(("copilot-usage-account", index))
                    .label(format!(
                        "{} / {} · {provider}",
                        account.username, account.org
                    ))
                    .selected(self.selected == index)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = index;
                        this.clear();
                        cx.notify();
                    })),
            );
        }
        let mut view = v_flex()
            .id("copilot-usage-scroll")
            .size_full()
            .gap_3()
            .overflow_y_scroll()
            .child(div().text_lg().child("Usage and billing"))
            .child(accounts)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("copilot-usage-refresh")
                            .label(if self.busy {
                                "Loading…"
                            } else {
                                "Refresh usage"
                            })
                            .disabled(self.busy || self.account().is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("copilot-usage-reload-accounts")
                            .label("Reload accounts from Settings")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.activate(cx);
                                cx.notify();
                            })),
                    ),
            );
        if self.config.accounts.is_empty() {
            view = view.child(div().text_color(muted).child(
                "Add GitHub accounts and choose their usage provider in Settings → Accounts.",
            ));
        }
        if let Some(pool) = &self.pool {
            view = view
                .child(format!(
                    "AI Credits: {} used / {} included · Estimated overage: ${:.2}",
                    pool.used,
                    pool.allotment,
                    pool.overage_cost()
                ))
                .child(div().text_color(muted).child(format!(
                    "Resets {}",
                    pool.reset_at.format("%Y-%m-%d %H:%M UTC")
                )));
            if let Some(projection) = copilot_usage::compute_projection(pool, Utc::now()) {
                view = view.child(format!(
                    "Projected period total: {} credits · Projected overage: ${:.2}",
                    projection.projected_total, projection.projected_overage_cost
                ));
            }
        }
        if let Some(billing) = &self.billing {
            if let Some(projection) = copilot_usage::budget::projected_spend(
                billing.usage.net_cost,
                billing.year,
                billing.month,
                billing.fetched_at,
            ) {
                view = view.child(format!("Projected period spend: ${projection:.2}"));
            }
            if billing.prevent_further_usage {
                view = view.child(
                    div()
                        .text_color(muted)
                        .child("This budget prevents further usage when its limit is reached."),
                );
            }
            view=view.child(format!("Billing period {}-{:02} · Gross ${:.2} · Discount ${:.2} · Net spend ${:.2} · {:.0} seats",billing.year,billing.month,billing.usage.gross_cost,billing.usage.discount,billing.usage.net_cost,billing.usage.business_seats))
                .child(match billing.budget_amount {Some(amount)=>format!("Monthly Copilot budget: ${amount:.2}"),None if billing.budget_unavailable=>"Budget unavailable. Billing spend remains available.".into(),None=>"No Copilot budget was returned.".into()});
        }
        if let Some(codex) = &self.codex {
            view = view.child(format!(
                "Local Codex login · Plan: {}",
                codex.plan_type.as_deref().unwrap_or("Unknown")
            ));
            for window in &codex.windows {
                view = view.child(format!(
                    "{}: {:.1}% used · {:.1}% remaining · Projected {:.1}% · Resets {}",
                    window.label,
                    window.used_percent,
                    window.remaining_percent,
                    window.projected_percent,
                    window.reset_at
                ));
            }
        }
        if let Some(error) = &self.error {
            view = view.child(div().text_color(danger).child(error.clone()));
        }
        if let Some(notice) = &self.notice {
            view = view.child(div().text_color(muted).child(notice.clone()));
        }
        view = view
            .child(div().text_lg().child("Copilot billing history"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .w(zpx(100.0))
                            .child(Input::new(&self.year).disabled(self.busy)),
                    )
                    .child(
                        div()
                            .w(zpx(90.0))
                            .child(Input::new(&self.month).disabled(self.busy)),
                    )
                    .child(
                        Button::new("copilot-usage-history-load")
                            .label("Load period")
                            .disabled(self.busy || self.account().is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.load_history(false, cx))),
                    )
                    .child(
                        Button::new("copilot-usage-history-older")
                            .label("Older snapshots")
                            .disabled(
                                self.busy || self.history.as_ref().is_none_or(|page| page.is_done),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.load_history(true, cx))),
                    ),
            );
        if let Some((year, month)) = self.history_period {
            view = view.child(
                div()
                    .text_color(muted)
                    .child(format!("Showing {year}-{month:02}")),
            );
        }
        if let Some(history) = &self.history {
            if history.page.is_empty() {
                view =
                    view.child("No billing snapshots were recorded for this account and period.");
            }
            for row in &history.page {
                let time = chrono::DateTime::from_timestamp_millis(row.snapshot_at as i64)
                    .map(|date| date.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "Unknown date".into());
                view=view.child(format!("{time} · {:.0} credits · Gross ${:.2} · Discount ${:.2} · Net ${:.2} · Spend ${:.2} · {:.0} seats · Budget {}",row.premium_requests,row.gross_cost,row.discount,row.net_cost,row.spent,row.business_seats,row.budget_amount.map(|amount|format!("${amount:.2}")).unwrap_or_else(||"unavailable".into())));
            }
        }
        if let Some(error) = &self.history_error {
            view = view.child(div().text_color(danger).child(error.clone()));
        }
        view.child(self.enterprise.clone())
    }
}
