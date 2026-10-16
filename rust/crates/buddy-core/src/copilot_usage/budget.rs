use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

pub struct Budget {
    pub amount: f64,
    pub blocked: bool,
}
fn pick(value: &Value, entity: Option<&str>) -> Option<Budget> {
    let rows = value["budgets"].as_array()?;
    let matches = |row: &&Value| {
        entity.is_none_or(|entity| {
            let name = row["budget_entity_name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let entity = entity.to_ascii_lowercase();
            !name.is_empty() && (name.contains(&entity) || entity.contains(&name))
        })
    };
    let rows: Vec<_> = rows.iter().filter(matches).collect();
    let kind = |row: &&Value, word: &str| {
        row["budget_product_sku"]
            .as_str()
            .is_some_and(|sku| sku.to_ascii_lowercase().contains(word))
    };
    let row = rows
        .iter()
        .find(|row| kind(row, "premium"))
        .or_else(|| rows.iter().find(|row| kind(row, "copilot")))?;
    Some(Budget {
        amount: row["budget_amount"]
            .as_f64()
            .filter(|amount| amount.is_finite() && *amount >= 0.0)?,
        blocked: row["prevent_further_usage"].as_bool().unwrap_or(false),
    })
}
async fn pages<F, Fut>(mut fetch: F, entity: Option<&str>) -> Result<Option<Budget>, ()>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Value, ()>>,
{
    for page in 1..=10 {
        let value = fetch(page).await?;
        if !value["budgets"].is_array() {
            return Err(());
        }
        if let Some(budget) = pick(&value, entity) {
            return Ok(Some(budget));
        }
        if value["has_next_page"] != true {
            return Ok(None);
        }
    }
    Err(())
}
pub(super) async fn fetch(
    org: &str,
    enterprise: Option<&str>,
    reader: &super::transport::Reader,
) -> (Option<Budget>, bool) {
    let org_result = pages(
        |page| async move {
            let body = reader
                .api(
                    &format!("/organizations/{org}/settings/billing/budgets?page={page}"),
                    &["X-GitHub-Api-Version: 2022-11-28"],
                )
                .await
                .map_err(|_| ())?;
            serde_json::from_str(&body).map_err(|_| ())
        },
        None,
    )
    .await;
    if let Ok(Some(budget)) = org_result {
        return (Some(budget), false);
    }
    if let Some(enterprise) = enterprise.filter(|value| !value.trim().is_empty()) {
        if !crate::gh::is_valid_slug(enterprise) {
            return (None, true);
        }
        match pages(
            |page| async move {
                let body = reader
                    .api(
                        &format!("/enterprises/{enterprise}/settings/billing/budgets?page={page}"),
                        &["X-GitHub-Api-Version: 2022-11-28"],
                    )
                    .await
                    .map_err(|_| ())?;
                serde_json::from_str(&body).map_err(|_| ())
            },
            Some(org),
        )
        .await
        {
            Ok(Some(budget)) => return (Some(budget), false),
            Err(()) => return (None, true),
            Ok(None) => {}
        }
    }
    (None, org_result.is_err())
}
/// Match Electron's dollar projection, including its one-second minimum and
/// refusal to extrapolate a different or expired billing period.
pub fn projected_spend(spent: f64, year: i32, month: u32, as_of: DateTime<Utc>) -> Option<f64> {
    if !spent.is_finite()
        || spent <= 0.0
        || !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
    {
        return None;
    }
    let start = Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()?;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()?;
    let elapsed = (as_of - start).num_milliseconds();
    if elapsed < 1000 || as_of >= end {
        return None;
    }
    let projection = spent * (end - start).num_milliseconds() as f64 / elapsed as f64;
    projection.is_finite().then_some(projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn enterprise_pagination_keeps_org_filter_and_premium_priority() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let result=pages(|page| {calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);std::future::ready(Ok(json!({"has_next_page":page==1,"budgets":if page==1 {json!([{"budget_entity_name":"other","budget_product_sku":"premium","budget_amount":999}])}else{json!([
            {"budget_entity_name":"My-Team","budget_product_sku":"copilot","budget_amount":100},
            {"budget_entity_name":"My-Team","budget_product_sku":"premium_requests","budget_amount":200,"prevent_further_usage":true}])}})))},Some("team")).await.unwrap().unwrap();
        assert_eq!(result.amount, 200.0);
        assert!(result.blocked);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(
            pages(
                |_| std::future::ready(Ok(json!({"budgets":[],"has_next_page":true}))),
                None
            )
            .await
            .is_err()
        );
        assert!(
            pages(|_| std::future::ready(Ok(json!({}))), None)
                .await
                .is_err()
        );
    }
    #[test]
    fn dollar_projection_obeys_billing_period_and_elapsed_second_boundaries() {
        let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
        let middle = Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap();
        assert_eq!(projected_spend(50.0, 2026, 10, middle), Some(100.0));
        assert_eq!(projected_spend(50.0, 2026, 10, start), None);
        assert_eq!(
            projected_spend(50.0, 2026, 10, start - chrono::Duration::days(1)),
            None
        );
        assert_eq!(
            projected_spend(
                50.0,
                2026,
                10,
                super::super::first_of_next_month_utc(2026, 10)
            ),
            None
        );
        assert_eq!(projected_spend(0.0, 2026, 10, middle), None);
    }
}
