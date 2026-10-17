//! Pure Codex allowance parsing, matching the existing Electron provider.
//! Credential access and HTTP transport belong to a separate boundary.
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

mod transport;
pub use transport::{FetchError, fetch};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const FIVE_HOURS: f64 = 18_000.0;
const WEEK: f64 = 604_800.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowKind {
    Weekly,
    FiveHour,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub kind: WindowKind,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub reset_at: String,
    pub duration_seconds: f64,
    pub period_start: String,
    pub projected_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub plan_type: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    InvalidResponse,
    NoWindows,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "The Codex usage response exceeds the size limit.",
            Self::InvalidResponse => "Could not parse the Codex usage response.",
            Self::NoWindows => "No Codex allowance windows are available for this account.",
        })
    }
}
impl std::error::Error for ParseError {}

fn time(milliseconds: f64) -> Option<(String, i64)> {
    if !milliseconds.is_finite() {
        return None;
    }
    let date = DateTime::<Utc>::from_timestamp_millis(milliseconds as i64)?;
    Some((
        date.to_rfc3339_opts(SecondsFormat::Millis, true),
        date.timestamp_millis(),
    ))
}

fn window(value: &Value, now: i64) -> Option<UsageWindow> {
    let used = value["used_percent"].as_f64().filter(|n| n.is_finite())?;
    let reset = value["reset_at"].as_f64().filter(|n| n.is_finite())?;
    let seconds = value["limit_window_seconds"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)?;
    let (reset_at, reset_ms) = time(reset * 1000.0)?;
    let (period_start, start_ms) = time(reset_ms as f64 - seconds * 1000.0)?;
    let used_percent = used.clamp(0.0, 100.0);
    let elapsed = ((now as f64 - start_ms as f64).max(0.0) / (seconds * 1000.0)).clamp(0.0, 1.0);
    let projected_percent = if elapsed > 0.0 {
        used_percent / elapsed
    } else {
        used_percent
    };
    if !projected_percent.is_finite() {
        return None;
    }
    let kind = if (seconds - WEEK).abs() <= WEEK * 0.05 {
        WindowKind::Weekly
    } else if (seconds - FIVE_HOURS).abs() <= FIVE_HOURS * 0.05 {
        WindowKind::FiveHour
    } else {
        WindowKind::Other
    };
    let label = match kind {
        WindowKind::Weekly => "Weekly allowance".into(),
        WindowKind::FiveHour => "5-hour allowance".into(),
        WindowKind::Other => format!(
            "{}-hour allowance",
            (seconds / 3600.0).round().max(1.0) as u64
        ),
    };
    Some(UsageWindow {
        kind,
        label,
        used_percent,
        remaining_percent: 100.0 - used_percent,
        reset_at,
        duration_seconds: seconds,
        period_start,
        projected_percent,
    })
}

/// The caller supplies time, so projections and expired/future windows are
/// reproducible without reading credentials or making provider requests.
pub fn parse(payload: &[u8], now: i64) -> Result<UsageSnapshot, ParseError> {
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let value: Value = serde_json::from_slice(payload).map_err(|_| ParseError::InvalidResponse)?;
    if !value.is_object() {
        return Err(ParseError::InvalidResponse);
    }
    let mut windows: Vec<_> = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|name| window(&value["rate_limit"][name], now))
        .collect();
    windows.sort_by_key(|window| window.kind);
    if windows.is_empty() {
        return Err(ParseError::NoWindows);
    }
    let plan_type = value["plan_type"].as_str().map(str::to_string);
    if plan_type.as_ref().is_some_and(|plan| plan.len() > 1024) {
        return Err(ParseError::InvalidResponse);
    }
    Ok(UsageSnapshot {
        plan_type,
        windows,
        fetched_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(used: Value, seconds: Value, reset: Value) -> Value {
        json!({"used_percent":used,"limit_window_seconds":seconds,"reset_at":reset})
    }
    fn response(first: Value, second: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"plan_type":"fixture-plan","rate_limit":{"primary_window":first,"secondary_window":second}})).unwrap()
    }

    #[test]
    fn weekly_precedes_five_hour_and_projection_uses_injected_time() {
        let payload = response(
            raw(json!(25), json!(18000), json!(18000)),
            raw(json!(14), json!(604800), json!(604800)),
        );
        let usage = parse(&payload, 7_200_000).unwrap();
        assert_eq!(usage.plan_type.as_deref(), Some("fixture-plan"));
        assert_eq!(usage.fetched_at, 7_200_000);
        assert_eq!(usage.windows[0].kind, WindowKind::Weekly);
        let window = &usage.windows[1];
        assert_eq!(window.label, "5-hour allowance");
        assert_eq!(window.used_percent, 25.0);
        assert_eq!(window.remaining_percent, 75.0);
        assert_eq!(window.projected_percent, 62.5);
        assert_eq!(window.reset_at, "1970-01-01T05:00:00.000Z");
        assert_eq!(window.period_start, "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn weekly_projection_retains_existing_elapsed_period_math() {
        let usage = parse(
            &response(raw(json!(14), json!(604800), json!(604800)), Value::Null),
            86_400_000,
        )
        .unwrap();
        assert!((usage.windows[0].projected_percent - 98.0).abs() < 0.00001);
    }

    #[test]
    fn percentages_are_clamped_and_future_or_expired_periods_stay_finite() {
        for (used, now, want) in [
            (json!(200), -1, 100.0),
            (json!(-20), 0, 0.0),
            (json!(40), 99_000_000, 40.0),
        ] {
            let snapshot = parse(
                &response(raw(used, json!(18000), json!(18000)), Value::Null),
                now,
            )
            .unwrap();
            assert_eq!(snapshot.windows[0].used_percent, want);
            assert_eq!(snapshot.windows[0].projected_percent, want);
        }
    }

    #[test]
    fn duration_tolerance_and_unknown_duration_labels_match_existing_provider() {
        let snapshot = parse(
            &response(
                raw(json!(1), json!(18100), json!(20000)),
                raw(json!(2), json!(10800), json!(20000)),
            ),
            1,
        )
        .unwrap();
        assert_eq!(snapshot.windows[0].kind, WindowKind::FiveHour);
        assert_eq!(snapshot.windows[1].kind, WindowKind::Other);
        assert_eq!(snapshot.windows[1].label, "3-hour allowance");
    }

    #[test]
    fn a_bad_optional_window_does_not_discard_a_valid_window() {
        let snapshot = parse(
            &response(
                raw(json!(10), json!(18000), json!(18000)),
                raw(json!("private-fixture"), json!(3600), json!(18000)),
            ),
            1,
        )
        .unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].used_percent, 10.0);
    }

    #[test]
    fn rejects_non_objects_absent_windows_bad_types_and_unrepresentable_dates() {
        for payload in [b"null".as_slice(), b"false", b"[]", b"{private-fixture"] {
            assert_eq!(parse(payload, 0), Err(ParseError::InvalidResponse));
        }
        for value in [
            Value::Null,
            raw(json!(1), json!(0), json!(1)),
            raw(json!(1), json!(-1), json!(1)),
            raw(json!(1), json!(3600), json!(1e99)),
        ] {
            assert_eq!(
                parse(&response(value, Value::Null), 0),
                Err(ParseError::NoWindows)
            );
        }
        assert!(
            !ParseError::InvalidResponse
                .to_string()
                .contains("private-fixture")
        );
    }

    #[test]
    fn bounds_response_and_plan_text_before_ui_or_history_storage() {
        assert_eq!(
            parse(&vec![b' '; MAX_RESPONSE_BYTES + 1], 0),
            Err(ParseError::TooLarge)
        );
        let payload=serde_json::to_vec(&json!({"plan_type":"x".repeat(1025),"rate_limit":{"primary_window":raw(json!(1),json!(18000),json!(18000))}})).unwrap();
        assert_eq!(parse(&payload, 0), Err(ParseError::InvalidResponse));
    }
}
