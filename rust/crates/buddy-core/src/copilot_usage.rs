//! Copilot usage for the Command Center card: billing parsing
//! (`billingParsers.ts`), AI Credit pools and projections (`quotaUtils.ts`),
//! aggregation (`useCopilotUsage.ts`), and fetching through `gh`
//! (`githubHandlers.ts`).

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use serde::Deserialize;

use crate::config::{GitHubAccount, GitHubConfig, UsageProvider};
use crate::gh;

pub mod budget;
pub mod enterprise;
pub mod history;
mod transport;

/// Cost of one AI Credit beyond the included allotment (`quotaUtils.ts`).
pub const OVERAGE_COST_PER_CREDIT: f64 = 0.01;
const SECONDS_PER_DAY: f64 = 86_400.0;
pub const NO_ORG_POOL_ERROR: &str =
    "Per-account AI Credit data requires an organization with Copilot seats.";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandCenterSummary {
    pub account_count: usize,
    pub loading: bool,
    pub total_used: i64,
    pub total_overage_cost: f64,
    pub projected_total: Option<i64>,
    pub projected_overage_cost: Option<f64>,
}

impl CommandCenterSummary {
    pub fn has_accounts(&self) -> bool {
        self.account_count > 0
    }

    /// "No accounts configured" or "N connected account(s)".
    pub fn account_description(&self) -> String {
        if !self.has_accounts() {
            return "No accounts configured".to_string();
        }
        let plural = if self.account_count == 1 { "" } else { "s" };
        format!("{} connected account{plural}", self.account_count)
    }
}

// ── Billing usage (`/settings/billing/usage`) ──────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BillingUsageItem {
    pub date: String,
    pub product: String,
    pub sku: String,
    pub quantity: f64,
    pub gross_amount: f64,
    pub discount_amount: f64,
    pub net_amount: f64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct BillingUsageResponse {
    usage_items: Vec<BillingUsageItem>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedBillingUsage {
    pub premium_requests: i64,
    pub gross_cost: f64,
    pub discount: f64,
    pub net_cost: f64,
    pub business_seats: f64,
    pub seat_plan: String,
}

/// Copilot AI Credits is the only consumption SKU; the legacy Premium Request
/// SKU bills in a parallel unit and would inflate usage.
const COPILOT_USAGE_SKUS: [&str; 1] = ["Copilot AI Credits"];
const SEAT_SKU_TO_PLAN: [(&str, &str); 2] = [
    ("Copilot Enterprise", "enterprise"),
    ("Copilot Business", "business"),
];

pub fn round_cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `parseBillingUsage` for one billing period.
pub fn parse_billing_usage(
    items: &[BillingUsageItem],
    year: i32,
    month: u32,
) -> ParsedBillingUsage {
    let prefix = format!("{year}-{month:02}-");
    let period: Vec<&BillingUsageItem> = items
        .iter()
        .filter(|i| i.date.starts_with(&prefix))
        .collect();
    let copilot = |item: &&BillingUsageItem| item.product == "copilot";

    let premium: Vec<&BillingUsageItem> = period
        .iter()
        .copied()
        .filter(|i| copilot(i) && COPILOT_USAGE_SKUS.contains(&i.sku.as_str()))
        .collect();
    let seats: Vec<&BillingUsageItem> = period
        .iter()
        .copied()
        .filter(|i| copilot(i) && SEAT_SKU_TO_PLAN.iter().any(|(sku, _)| *sku == i.sku))
        .collect();
    let seat_plan = SEAT_SKU_TO_PLAN
        .iter()
        .find(|(sku, _)| {
            period
                .iter()
                .filter(|i| copilot(i) && i.sku == *sku)
                .map(|i| i.quantity)
                .sum::<f64>()
                > 0.0
        })
        .map(|(_, plan)| plan.to_string())
        .unwrap_or_default();

    ParsedBillingUsage {
        premium_requests: premium.iter().map(|i| i.quantity).sum::<f64>().round() as i64,
        gross_cost: round_cents(premium.iter().map(|i| i.gross_amount).sum()),
        discount: round_cents(premium.iter().map(|i| i.discount_amount).sum()),
        net_cost: round_cents(premium.iter().map(|i| i.net_amount).sum()),
        business_seats: round_cents(seats.iter().map(|i| i.quantity).sum()),
        seat_plan,
    }
}

/// AI Credits included per seat per month: GitHub's promotional 7,000 for
/// 2026-06..2026-08, otherwise 3,900.
pub fn credits_per_seat(year: i32, month: u32) -> i64 {
    if year == 2026 && (6..=8).contains(&month) {
        7000
    } else {
        3900
    }
}

pub fn credit_allotment(seats: f64, year: i32, month: u32) -> i64 {
    (seats * credits_per_seat(year, month) as f64).round() as i64
}

pub fn first_of_next_month_utc(year: i32, month: u32) -> DateTime<Utc> {
    let (y, m) = if month >= 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .expect("valid first-of-month date")
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let next = if month >= 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next.0, next.1, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

// ── Pools and projections ───────────────────────────────────────────────────

/// One org (or personal) AI Credit pool: the `premium_interactions` snapshot
/// reduced to what the dashboard needs.
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePool {
    /// `entitlement - remaining`; may exceed the allotment under overage.
    pub used: i64,
    pub allotment: i64,
    /// GitHub's own `overage_count`; zero for synthesized org pools.
    pub overage_count: i64,
    pub reset_at: DateTime<Utc>,
}

impl UsagePool {
    /// `computeOverageRequests`: the larger of `overage_count` and `-remaining`.
    pub fn overage_requests(&self) -> i64 {
        self.overage_count.max(self.used - self.allotment).max(0)
    }

    pub fn overage_cost(&self) -> f64 {
        self.overage_requests() as f64 * OVERAGE_COST_PER_CREDIT
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub projected_total: i64,
    pub projected_overage_cost: f64,
}

/// `computeProjection`: linear month-end projection in UTC with the elapsed
/// window floored at one day and capped at the full period.
pub fn compute_projection(pool: &UsagePool, now: DateTime<Utc>) -> Option<Projection> {
    let reset = pool.reset_at;
    let (year, month) = if reset.month() == 1 {
        (reset.year() - 1, 12)
    } else {
        (reset.year(), reset.month() - 1)
    };
    let day = reset.day().min(last_day_of_month(year, month));
    let period_start = Utc
        .with_ymd_and_hms(year, month, day, reset.hour(), reset.minute(), 0)
        .single()?;

    let total_seconds = (reset - period_start).num_seconds() as f64;
    let elapsed_seconds = (now - period_start).num_seconds() as f64;
    if elapsed_seconds < 1.0 || total_seconds < 1.0 {
        return None;
    }
    let effective_elapsed = elapsed_seconds.max(SECONDS_PER_DAY).min(total_seconds);
    let used = pool.used.max(0) as f64;
    let projected_total = (used / effective_elapsed * total_seconds).round() as i64;
    let projected_overage = (projected_total - pool.allotment).max(0);
    Some(Projection {
        projected_total,
        projected_overage_cost: projected_overage as f64 * OVERAGE_COST_PER_CREDIT,
    })
}

/// `computeAggregateTotals` + `computeAggregateProjections` over one
/// representative pool per org.
pub fn aggregate(
    pools: &[UsagePool],
    account_count: usize,
    now: DateTime<Utc>,
) -> CommandCenterSummary {
    let mut summary = CommandCenterSummary {
        account_count,
        ..Default::default()
    };
    let mut projected_total = 0;
    let mut projected_overage_cost = 0.0;
    let mut any_projection = false;
    for pool in pools {
        summary.total_used += pool.used;
        summary.total_overage_cost += pool.overage_cost();
        if let Some(projection) = compute_projection(pool, now) {
            any_projection = true;
            projected_total += projection.projected_total;
            projected_overage_cost += projection.projected_overage_cost;
        }
    }
    if any_projection {
        summary.projected_total = Some(projected_total);
        summary.projected_overage_cost = Some(projected_overage_cost);
    }
    summary
}

// ── Fetching through gh ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    quota_reset_date_utc: Option<String>,
    quota_snapshots: QuotaSnapshots,
}

#[derive(Debug, Deserialize)]
struct QuotaSnapshots {
    premium_interactions: PremiumSnapshot,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PremiumSnapshot {
    entitlement: f64,
    remaining: f64,
    /// May be absent or an explicit `null`; both mean "derive from remaining".
    overage_count: Option<f64>,
}

/// `/copilot_internal/user` for a personal namespace (username == org).
pub fn parse_personal_quota(json: &str, now: DateTime<Utc>) -> Result<UsagePool, String> {
    let response: QuotaResponse =
        serde_json::from_str(json).map_err(|_| "Unreadable Copilot quota response".to_string())?;
    let premium = response.quota_snapshots.premium_interactions;
    let reset_at = response
        .quota_reset_date_utc
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| first_of_next_month_utc(now.year(), now.month()));
    Ok(UsagePool {
        used: (premium.entitlement - premium.remaining).round() as i64,
        allotment: premium.entitlement.round() as i64,
        overage_count: premium.overage_count.unwrap_or(0.0).round().max(0.0) as i64,
        reset_at,
    })
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct CopilotBillingResponse {
    seat_breakdown: SeatBreakdown,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SeatBreakdown {
    total: Option<f64>,
}

fn is_personal_namespace(account: &GitHubAccount) -> bool {
    account.username.eq_ignore_ascii_case(&account.org)
}

async fn fetch_seat_count(org: &str, reader: &transport::Reader) -> Option<f64> {
    let body = reader
        .api(
            &format!("/orgs/{org}/copilot/billing"),
            &["X-GitHub-Api-Version: 2022-11-28"],
        )
        .await
        .ok()?;
    serde_json::from_str::<CopilotBillingResponse>(&body)
        .ok()?
        .seat_breakdown
        .total
}

/// `fetchOrgOrUserBillingUsage`: org endpoint, then user endpoint, with the
/// descriptive double-404 message.
async fn fetch_billing_usage_body(
    org: &str,
    year: i32,
    month: u32,
    reader: &transport::Reader,
) -> Result<String, String> {
    let query = format!("settings/billing/usage?year={year}&month={month}");
    match reader.api(&format!("/orgs/{org}/{query}"), &[]).await {
        Ok(body) => Ok(body),
        Err(err) if err.is_not_found() => {
            match reader.api(&format!("/users/{org}/{query}"), &[]).await {
                Ok(body) => Ok(body),
                Err(err) if err.is_not_found() => Err(format!(
                    "No billing access for '{org}'. This may be a user account — billing data requires the 'user' token scope, or '{org}' may not be a valid org."
                )),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Separate billing figures from the allowance pool: unavailable spend is never zero.
#[derive(Debug, Clone)]
pub struct BillingSnapshot {
    pub usage: ParsedBillingUsage,
    pub budget_amount: Option<f64>,
    pub budget_unavailable: bool,
    pub prevent_further_usage: bool,
    pub fetched_at: DateTime<Utc>,
    pub year: i32,
    pub month: u32,
}

pub async fn fetch_billing(
    account: &GitHubAccount,
    now: DateTime<Utc>,
    enterprise: Option<&str>,
) -> Result<BillingSnapshot, String> {
    gh::assert_valid_slug(&account.username).map_err(|_| "Invalid GitHub account.".to_string())?;
    gh::assert_valid_slug(&account.org)
        .map_err(|_| "Configure an organization to read billing usage.".to_string())?;
    let reader = transport::Reader::new(account).await?;
    let body = fetch_billing_usage_body(&account.org, now.year(), now.month(), &reader)
        .await
        .map_err(|_| {
            "Billing usage is unavailable for this account and organization.".to_string()
        })?;
    let response: BillingUsageResponse = serde_json::from_str(&body)
        .map_err(|_| "The billing usage response is invalid.".to_string())?;
    let (budget, budget_unavailable) = budget::fetch(&account.org, enterprise, &reader).await;
    Ok(BillingSnapshot {
        usage: parse_billing_usage(&response.usage_items, now.year(), now.month()),
        budget_amount: budget.as_ref().map(|budget| budget.amount),
        budget_unavailable,
        prevent_further_usage: budget.as_ref().is_some_and(|budget| budget.blocked),
        fetched_at: now,
        year: now.year(),
        month: now.month(),
    })
}

/// One account's AI Credit pool (`doFetchQuota`).
pub async fn fetch_account_pool(
    account: &GitHubAccount,
    now: DateTime<Utc>,
) -> Result<UsagePool, String> {
    gh::assert_valid_slug(&account.username).map_err(|e| e.to_string())?;
    if account.org.is_empty() {
        return Err(NO_ORG_POOL_ERROR.to_string());
    }
    gh::assert_valid_slug(&account.org).map_err(|e| e.to_string())?;

    let reader = transport::Reader::new(account).await?;

    if is_personal_namespace(account) {
        // `/copilot_internal/user` answers for whoever the token belongs to, so a
        // personal account must use its own credential; never fall back to the
        // active login and show another user's quota (`getCopilotQuota`).
        if !reader.selected() {
            return Err("Sign in with GitHub CLI for the selected account.".into());
        }
        let body = reader
            .api("/copilot_internal/user", &[])
            .await
            .map_err(|e| e.to_string())?;
        return parse_personal_quota(&body, now);
    }

    // Org billing: like `tryGetCliToken`, a missing per-account token falls
    // back to the active login, which may still have org access.

    let (year, month) = (now.year(), now.month());
    let body = fetch_billing_usage_body(&account.org, year, month, &reader).await?;
    let response: BillingUsageResponse =
        serde_json::from_str(&body).map_err(|_| "Unreadable billing usage response".to_string())?;
    let parsed = parse_billing_usage(&response.usage_items, year, month);
    let seats = fetch_seat_count(&account.org, &reader)
        .await
        .unwrap_or(parsed.business_seats);
    if seats <= 0.0 {
        return Err(NO_ORG_POOL_ERROR.to_string());
    }
    Ok(UsagePool {
        used: parsed.premium_requests,
        allotment: credit_allotment(seats, year, month),
        overage_count: 0,
        reset_at: first_of_next_month_utc(year, month),
    })
}

/// Result of refreshing every Copilot-billed account.
#[derive(Debug, Clone, Default)]
pub struct UsageReport {
    pub summary: CommandCenterSummary,
    /// `(username, error)` for accounts whose fetch failed.
    pub errors: Vec<(String, String)>,
}

/// `useCopilotUsage`: skip accounts whose effective provider is Codex (the
/// override map wins, as in Electron), fetch every remaining account, and
/// count each org pool once using the first account that produced data.
pub async fn fetch_report(github: &GitHubConfig, now: DateTime<Utc>) -> UsageReport {
    let accounts: Vec<&GitHubAccount> = github
        .accounts
        .iter()
        .filter(|a| github.effective_usage_provider(a) != Some(UsageProvider::Codex))
        .collect();
    let results =
        futures::future::join_all(accounts.iter().map(|a| fetch_account_pool(a, now))).await;

    let mut representatives: Vec<(String, UsagePool)> = Vec::new();
    let mut errors = Vec::new();
    for (account, result) in accounts.iter().zip(results) {
        match result {
            Ok(pool) => {
                let org = account.org.to_ascii_lowercase();
                if !representatives.iter().any(|(o, _)| *o == org) {
                    representatives.push((org, pool));
                }
            }
            Err(err) => errors.push((account.username.clone(), err)),
        }
    }
    let pools: Vec<UsagePool> = representatives.into_iter().map(|(_, p)| p).collect();
    UsageReport {
        summary: aggregate(&pools, accounts.len(), now),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(date: &str, sku: &str, quantity: f64, gross: f64, net: f64) -> BillingUsageItem {
        BillingUsageItem {
            date: date.into(),
            product: "copilot".into(),
            sku: sku.into(),
            quantity,
            gross_amount: gross,
            discount_amount: gross - net,
            net_amount: net,
        }
    }

    #[test]
    fn describes_accounts() {
        let mut summary = CommandCenterSummary::default();
        assert_eq!(summary.account_description(), "No accounts configured");
        summary.account_count = 1;
        assert_eq!(summary.account_description(), "1 connected account");
        summary.account_count = 3;
        assert_eq!(summary.account_description(), "3 connected accounts");
    }

    #[test]
    fn parses_billing_usage_counting_only_ai_credits_in_period() {
        let items = vec![
            item("2026-10-01", "Copilot AI Credits", 1200.0, 12.0, 0.0),
            item("2026-10-02", "Copilot AI Credits", 84.4, 0.844, 0.844),
            item("2026-10-02", "Copilot Premium Request", 500.0, 20.0, 20.0),
            item("2026-10-01", "Copilot Business", 5.0, 95.0, 95.0),
            item("2026-09-30", "Copilot AI Credits", 999.0, 9.99, 9.99),
        ];
        let parsed = parse_billing_usage(&items, 2026, 10);
        assert_eq!(parsed.premium_requests, 1284);
        assert_eq!(parsed.gross_cost, 12.84);
        assert_eq!(parsed.net_cost, 0.84);
        assert_eq!(parsed.business_seats, 5.0);
        assert_eq!(parsed.seat_plan, "business");
    }

    #[test]
    fn allotments_follow_the_promo_window() {
        assert_eq!(credits_per_seat(2026, 7), 7000);
        assert_eq!(credits_per_seat(2026, 10), 3900);
        assert_eq!(credit_allotment(5.0, 2026, 10), 19_500);
        assert_eq!(
            first_of_next_month_utc(2026, 12).to_rfc3339(),
            "2027-01-01T00:00:00+00:00"
        );
    }

    #[test]
    fn projects_linearly_to_month_end() {
        // Period Oct 1 .. Nov 1; 10 days elapsed with 1,000 used -> 3,100 for 31 days.
        let pool = UsagePool {
            used: 1000,
            allotment: 3900,
            overage_count: 0,
            reset_at: first_of_next_month_utc(2026, 10),
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 0, 0, 0).unwrap();
        let projection = compute_projection(&pool, now).unwrap();
        assert_eq!(projection.projected_total, 3100);
        assert_eq!(projection.projected_overage_cost, 0.0);

        // Early in the cycle the elapsed window is floored at one day.
        let early = Utc.with_ymd_and_hms(2026, 10, 1, 1, 0, 0).unwrap();
        assert_eq!(
            compute_projection(&pool, early).unwrap().projected_total,
            31_000
        );

        // Overage is billed at one cent per credit.
        let heavy = UsagePool {
            used: 4000,
            ..pool.clone()
        };
        let projection = compute_projection(&heavy, now).unwrap();
        assert_eq!(projection.projected_total, 12_400);
        assert!((projection.projected_overage_cost - 85.0).abs() < 1e-9);

        // Before the period starts there is nothing to project.
        let before = Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap();
        assert!(compute_projection(&pool, before).is_none());
    }

    #[test]
    fn month_arithmetic_clamps_day_overflow() {
        // Reset Mar 31 -> period start Feb 28 (not Mar 3).
        let pool = UsagePool {
            used: 0,
            allotment: 0,
            overage_count: 0,
            reset_at: Utc.with_ymd_and_hms(2026, 3, 31, 0, 0, 0).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        assert!(compute_projection(&pool, now).is_some());
        assert_eq!(last_day_of_month(2026, 2), 28);
        assert_eq!(last_day_of_month(2028, 2), 29);
    }

    #[test]
    fn parses_personal_quota_snapshot() {
        let json = r#"{"login":"hemsoft","quota_reset_date_utc":"2026-11-01T00:00:00Z",
          "quota_snapshots":{"chat":{},"completions":{},
            "premium_interactions":{"entitlement":3900,"remaining":2616,"overage_count":0,"credits_used":1284}}}"#;
        let now = Utc.with_ymd_and_hms(2026, 10, 7, 0, 0, 0).unwrap();
        let pool = parse_personal_quota(json, now).unwrap();
        assert_eq!(pool.used, 1284);
        assert_eq!(pool.allotment, 3900);
        assert_eq!(pool.overage_count, 0);

        // An explicit null is accepted and derives overage from `remaining`.
        let nulled = r#"{"quota_snapshots":{"premium_interactions":{"entitlement":100,"remaining":-5,"overage_count":null}}}"#;
        assert_eq!(
            parse_personal_quota(nulled, now)
                .unwrap()
                .overage_requests(),
            5
        );

        // GitHub's overage_count is preserved even when `remaining` says zero.
        let overage = r#"{"quota_snapshots":{"premium_interactions":{"entitlement":100,"remaining":0,"overage_count":100}}}"#;
        let pool = parse_personal_quota(overage, now).unwrap();
        assert_eq!(pool.overage_requests(), 100);
        assert!((pool.overage_cost() - 1.0).abs() < 1e-9);
        assert_eq!(pool.reset_at.to_rfc3339(), "2026-11-01T00:00:00+00:00");
    }

    #[test]
    fn aggregates_pools_and_projections() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 0, 0, 0).unwrap();
        let a = UsagePool {
            used: 1000,
            allotment: 3900,
            overage_count: 0,
            reset_at: first_of_next_month_utc(2026, 10),
        };
        let b = UsagePool {
            used: 4500,
            allotment: 3900,
            overage_count: 0,
            reset_at: first_of_next_month_utc(2026, 10),
        };
        let summary = aggregate(&[a, b], 2, now);
        assert_eq!(summary.account_count, 2);
        assert_eq!(summary.total_used, 5500);
        assert!((summary.total_overage_cost - 6.0).abs() < 1e-9);
        assert_eq!(summary.projected_total, Some(3100 + 13_950));
        assert!(summary.projected_overage_cost.unwrap() > 0.0);
        let empty = aggregate(&[], 0, now);
        assert_eq!(empty.projected_total, None);
    }
}
