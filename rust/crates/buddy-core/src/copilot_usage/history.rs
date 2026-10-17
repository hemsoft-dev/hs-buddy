//! Bounded pages over the same immutable billing snapshots used by Electron.
use super::BillingSnapshot;
use crate::config::GitHubAccount;
use crate::terminal_backend::TerminalBackend;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    #[serde(deserialize_with = "finite_number")]
    pub snapshot_at: f64,
    #[serde(deserialize_with = "finite_number")]
    pub premium_requests: f64,
    #[serde(deserialize_with = "finite_number")]
    pub gross_cost: f64,
    #[serde(deserialize_with = "finite_number")]
    pub discount: f64,
    #[serde(deserialize_with = "finite_number")]
    pub net_cost: f64,
    #[serde(deserialize_with = "finite_number")]
    pub business_seats: f64,
    #[serde(default, deserialize_with = "optional_number")]
    pub budget_amount: Option<f64>,
    #[serde(deserialize_with = "finite_number")]
    pub spent: f64,
}
// Convex encodes negative zero as a typed float. Empty billing sums can produce
// that value, including in snapshots already stored by earlier versions.
fn number(value: serde_json::Value) -> Option<f64> {
    match convex::Value::try_from(value).ok()? {
        convex::Value::Float64(number) if number.is_finite() => Some(number),
        _ => None,
    }
}
fn finite_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    number(serde_json::Value::deserialize(deserializer)?)
        .ok_or_else(|| serde::de::Error::custom("Invalid billing number"))
}
fn optional_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    Option::<serde_json::Value>::deserialize(deserializer)?
        .map(|value| {
            number(value).ok_or_else(|| serde::de::Error::custom("Invalid billing number"))
        })
        .transpose()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub page: Vec<Row>,
    pub continue_cursor: String,
    pub is_done: bool,
    pub page_status: Option<String>,
}
#[derive(Clone)]
pub struct History {
    backend: TerminalBackend,
}
impl History {
    pub fn new(url: &str) -> Result<Self, String> {
        Ok(Self {
            backend: TerminalBackend::new(url)
                .map_err(|_| "Usage history service is unavailable.".to_string())?,
        })
    }
    pub async fn page(
        &self,
        account: &GitHubAccount,
        year: i32,
        month: u32,
        cursor: Option<&str>,
    ) -> Result<Page, String> {
        if !(1..=12).contains(&month)
            || !(1970..=9999).contains(&year)
            || cursor.is_some_and(|cursor| cursor.len() > 16384)
        {
            return Err("Choose a valid billing period.".into());
        }
        let value=self.backend.call("copilotNative:listUsage",json!({"accountUsername":account.username,"org":account.org,"billingYear":year,"billingMonth":month,"paginationOpts":{"numItems":10,"cursor":cursor}}),false).await.map_err(|_|"Usage history is unavailable. Try refreshing.".to_string())?;
        parse_page(value, cursor)
    }
    /// One write, with no automatic retry: a lost reply can already have stored the snapshot.
    pub async fn store(
        &self,
        account: &GitHubAccount,
        billing: &BillingSnapshot,
    ) -> Result<(), String> {
        store(&self.backend, account, billing).await
    }
}
fn parse_page(value: serde_json::Value, cursor: Option<&str>) -> Result<Page, String> {
    if serde_json::to_vec(&value)
        .map_err(|_| "Usage history is invalid.")?
        .len()
        > 128 * 1024
    {
        return Err("Usage history exceeds the page limit.".into());
    }
    let page: Page =
        serde_json::from_value(value).map_err(|_| "Usage history is invalid.".to_string())?;
    if page.page.len() > 25
        || page.continue_cursor.len() > 16384
        || page
            .page_status
            .as_ref()
            .is_some_and(|status| status != "SplitRecommended")
        || (!page.is_done
            && (page.continue_cursor.is_empty() || Some(page.continue_cursor.as_str()) == cursor))
        || page.page.iter().any(|row| {
            !row.snapshot_at.is_finite()
                || row.snapshot_at.fract() != 0.0
                || !(0.0..=253_402_300_799_999.0).contains(&row.snapshot_at)
        })
    {
        return Err("This usage history page cannot be displayed. Choose another period.".into());
    }
    Ok(page)
}
async fn store(
    backend: &TerminalBackend,
    account: &GitHubAccount,
    billing: &BillingSnapshot,
) -> Result<(), String> {
    let usage = &billing.usage;
    let mut args = json!({"accountUsername":account.username,"org":account.org,"billingYear":billing.year,"billingMonth":billing.month,"premiumRequests":usage.premium_requests,"grossCost":usage.gross_cost,"discount":usage.discount,"netCost":usage.net_cost,"businessSeats":usage.business_seats,"spent":usage.net_cost});
    if let Some(amount) = billing.budget_amount {
        args["budgetAmount"] = json!(amount);
    }
    let value = backend
        .call("copilotUsageHistory:store", args, true)
        .await
        .map_err(|_| {
            "The billing snapshot save is unconfirmed. Refresh history to check it.".to_string()
        })?;
    if value.as_str().is_none() {
        return Err(
            "The billing snapshot save is unconfirmed. Refresh history to check it.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_seat_skus_survive_the_billing_store_and_native_history_round_trip() {
        let billing = super::super::parse_billing_usage(
            &[super::super::BillingUsageItem {
                date: "2026-10-02".into(),
                product: "copilot".into(),
                sku: "Copilot AI Credits".into(),
                quantity: 1200.0,
                gross_amount: 12.0,
                discount_amount: 3.0,
                net_amount: 9.0,
            }],
            2026,
            10,
        );
        assert!(billing.business_seats.is_sign_negative());
        let value: serde_json::Value = convex::Value::try_from(json!({"page":[{
            "snapshotAt":1792152000123.0,"premiumRequests":billing.premium_requests as f64,
            "grossCost":billing.gross_cost,"discount":billing.discount,"netCost":billing.net_cost,
            "businessSeats":billing.business_seats,"spent":billing.net_cost
        }],"continueCursor":"done","isDone":true}))
        .unwrap()
        .into();
        assert!(value["page"][0]["businessSeats"].get("$float").is_some());
        let page = parse_page(value, None).unwrap();
        assert_eq!(page.page[0].business_seats, 0.0);
    }
    #[test]
    fn native_convex_float_timestamps_survive_the_actual_value_conversion() {
        let value:serde_json::Value=convex::Value::try_from(json!({"page":[{"snapshotAt":1792152000123.0,"premiumRequests":12.0,"grossCost":2.0,"discount":1.0,"netCost":1.0,"businessSeats":1.0,"spent":1.0}],"continueCursor":"done","isDone":true})).unwrap().into();
        let page = parse_page(value, None).unwrap();
        assert_eq!(page.page[0].snapshot_at, 1792152000123.0);
    }
    #[test]
    fn typed_billing_numbers_accept_negative_zero_but_reject_other_types_and_nonfinite_values() {
        let row = json!({"snapshotAt":1792152000123.0,"premiumRequests":12.0,"grossCost":2.0,"discount":1.0,"netCost":1.0,"businessSeats":1.0,"budgetAmount":200.0,"spent":1.0});
        for field in [
            "snapshotAt",
            "premiumRequests",
            "grossCost",
            "discount",
            "netCost",
            "businessSeats",
            "budgetAmount",
            "spent",
        ] {
            for (value, valid) in [
                (convex::Value::Float64(-0.0).into(), true),
                (convex::Value::Float64(f64::NAN).into(), false),
                (convex::Value::Float64(f64::INFINITY).into(), false),
                (convex::Value::Float64(f64::NEG_INFINITY).into(), false),
                (convex::Value::Int64(0).into(), false),
                (convex::Value::Bytes(vec![0]).into(), false),
                (json!("0"), false),
            ] {
                let mut candidate = row.clone();
                candidate[field] = value;
                assert_eq!(
                    parse_page(
                        json!({"page":[candidate],"continueCursor":"done","isDone":true}),
                        None
                    )
                    .is_ok(),
                    valid,
                    "field {field}"
                );
            }
        }
    }
    #[test]
    fn incomplete_unknown_and_repeated_cursor_pages_cannot_be_presented_as_complete() {
        for value in [
            json!({"page":[],"continueCursor":"same","isDone":false}),
            json!({"page":[],"continueCursor":"next","isDone":false,"pageStatus":"SplitRequired"}),
            json!({"page":[],"continueCursor":"next","isDone":false,"pageStatus":"future"}),
        ] {
            assert!(parse_page(value, Some("same")).is_err());
        }
    }
}
