use super::*;
use serde_json::json;

/// 2026-11-28T12:00:00Z: Saturday 07:00 in New York.
fn now() -> Timestamp {
    Timestamp::from_millisecond(1_795_867_200_000).unwrap()
}

fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap()
}

fn schedule(id: &str, cron: &str, timezone: Option<&str>, enabled: bool) -> Schedule {
    let mut value = json!({"_id":id,"jobId":format!("job-{id}"),"name":format!("Schedule {id}"),
        "cron":cron,"enabled":enabled,"missedPolicy":"skip"});
    if let Some(timezone) = timezone {
        value["timezone"] = json!(timezone);
    }
    serde_json::from_value(value).unwrap()
}

fn rows(forecast: &Forecast) -> Vec<(String, Vec<(String, String)>)> {
    forecast
        .groups
        .iter()
        .map(|group| {
            (
                group.label.clone(),
                group
                    .occurrences
                    .iter()
                    .map(|occurrence| {
                        (
                            occurrence.at.strftime("%H:%M").to_string(),
                            occurrence.schedule_id.clone(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn pair(time: &str, id: &str) -> (String, String) {
    (time.into(), id.into())
}

#[test]
fn occurrences_group_by_the_viewers_day_with_electrons_labels_and_counts() {
    let schedules = [
        schedule("a", "0 9 * * *", Some("America/New_York"), true),
        // No timezone: read in the viewer's zone, as Electron does.
        schedule("b", "30 8 * * *", None, true),
        schedule("paused", "* * * * *", Some("UTC"), false),
        schedule("bad-cron", "not a cron", Some("UTC"), true),
        schedule("bad-zone", "0 9 * * *", Some("Nowhere/Invalid"), true),
    ];
    let forecast = forecast(&schedules, now(), &zone("America/New_York"), 3.0);
    assert_eq!(
        rows(&forecast),
        [
            (
                "Today".to_owned(),
                vec![pair("08:30", "b"), pair("09:00", "a")]
            ),
            (
                "Tomorrow".to_owned(),
                vec![pair("08:30", "b"), pair("09:00", "a")]
            ),
            (
                "Monday, Nov 30".to_owned(),
                vec![pair("08:30", "b"), pair("09:00", "a")]
            ),
        ]
    );
    // The window is three times 24 hours: it ends at 07:00 on December 1.
    assert_eq!(forecast.runs(), 6);
    assert_eq!(
        (forecast.schedules, forecast.active, forecast.paused),
        (5, 4, 1)
    );
    assert_eq!(
        forecast.summary(),
        ["4 active schedules", "1 paused", "6 runs in next 3 days"]
    );
    let first = &forecast.groups[0].occurrences[1];
    assert_eq!(
        (
            first.schedule_name.as_str(),
            first.job_id.as_str(),
            first.cron.as_str()
        ),
        ("Schedule a", "job-a", "0 9 * * *")
    );
}

#[test]
fn the_viewers_zone_decides_the_day_and_the_time_shown() {
    let schedules = [
        schedule("ny", "0 21 * * *", Some("America/New_York"), true),
        schedule("local", "0 9 * * *", None, true),
    ];
    // From Tokyo it is already Saturday 21:00. Saturday 21:00 in New York is
    // Sunday 11:00 there, and the zoneless schedule runs at 09:00 Tokyo time.
    let forecast = forecast(&schedules, now(), &zone("Asia/Tokyo"), 1.0);
    assert_eq!(
        rows(&forecast),
        [(
            "Tomorrow".to_owned(),
            vec![pair("09:00", "local"), pair("11:00", "ny")]
        )]
    );
    assert_eq!(forecast.summary()[1], "2 runs in next 1 day");
    // From New York the same instant is Saturday 07:00: both runs are today.
    let forecast = super::forecast(&schedules, now(), &zone("America/New_York"), 1.0);
    assert_eq!(
        rows(&forecast),
        [(
            "Today".to_owned(),
            vec![pair("09:00", "local"), pair("21:00", "ny")]
        )]
    );
}

#[test]
fn schedules_at_the_same_minute_keep_their_order_and_each_is_capped() {
    let schedules = [
        schedule("second", "0 * * * *", Some("UTC"), true),
        schedule("first", "0 * * * *", Some("UTC"), true),
        schedule("busy", "* * * * *", Some("UTC"), true),
    ];
    let forecast = forecast(&schedules, now(), &zone("UTC"), 1.0);
    let ids: Vec<&str> = forecast
        .groups
        .iter()
        .flat_map(|group| &group.occurrences)
        .filter(|occurrence| occurrence.at.minute() == 0)
        .map(|occurrence| occurrence.schedule_id.as_str())
        .take(3)
        .collect();
    // 13:00 is the first full hour after 12:00; ties keep the list order.
    assert_eq!(ids, ["second", "first", "busy"]);
    let busy = forecast
        .groups
        .iter()
        .flat_map(|group| &group.occurrences)
        .filter(|occurrence| occurrence.schedule_id == "busy")
        .count();
    assert_eq!(busy, PER_SCHEDULE_LIMIT, "1440 minutes are capped at 500");
}

#[test]
fn empty_forecasts_explain_why() {
    let viewer = zone("UTC");
    let nothing = forecast(&[], now(), &viewer, 1.0);
    assert!(nothing.groups.is_empty());
    assert_eq!(
        nothing.empty(),
        (
            "No scheduled runs in the next 1 day.".to_owned(),
            Some("Create a schedule to get started.")
        )
    );
    assert_eq!(
        nothing.summary(),
        ["0 active schedules", "0 runs in next 1 day"]
    );
    let paused = forecast(
        &[schedule("p", "* * * * *", None, false)],
        now(),
        &viewer,
        2.0,
    );
    assert_eq!(
        paused.empty(),
        (
            "No scheduled runs in the next 2 days.".to_owned(),
            Some("All schedules are currently paused.")
        )
    );
    // Active, but its next run is on January 1.
    let later = forecast(
        &[schedule("y", "@yearly", Some("UTC"), true)],
        now(),
        &viewer,
        30.0,
    );
    assert!(later.groups.is_empty());
    assert_eq!(later.empty().1, None);
    assert_eq!(later.summary()[0], "1 active schedule");
}

#[test]
fn forecast_days_outside_one_to_thirty_fall_back_to_three() {
    for (stored, used) in [
        (1.0, 1.0),
        (30.0, 30.0),
        (2.5, 2.5),
        (0.0, 3.0),
        (31.0, 3.0),
        (-4.0, 3.0),
        (f64::NAN, 3.0),
        (f64::INFINITY, 3.0),
    ] {
        assert_eq!(normalize_days(stored), used, "{stored}");
    }
    assert_eq!(count(1.0, "day"), "1 day");
    assert_eq!(count(2.5, "day"), "2.5 days");
    assert_eq!(count(0.0, "run"), "0 runs");
    let forecast = forecast(&[], now(), &zone("UTC"), 99.0);
    assert_eq!(forecast.days, 3.0);
    assert_eq!(DAY_OPTIONS, [1, 2, 3, 5, 7, 14, 30]);
}
