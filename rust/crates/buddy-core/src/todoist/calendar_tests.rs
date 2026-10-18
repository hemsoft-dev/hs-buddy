use chrono::NaiveDate;
use serde_json::json;

use super::*;
use crate::todoist::dates::RawDue;
use crate::todoist::{DayKind, Id};

fn calendar(zone: Option<&str>) -> AccountCalendar {
    AccountCalendar::from_profile(&Profile {
        id: Id::parse("synthetic-user").unwrap(),
        timezone: zone.map(str::to_owned),
    })
    .unwrap()
}
fn due(text: &str) -> DueDate {
    let raw: RawDue = serde_json::from_value(json!({"date": text})).unwrap();
    DueDate::parse(raw).unwrap()
}
fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

#[test]
fn absent_or_unsupported_account_zones_do_not_fall_back_to_the_machine() {
    for zone in [
        None,
        Some(""),
        Some("Synthetic/Not_A_Zone"),
        Some("Etc/Unknown"),
    ] {
        let profile = Profile {
            id: Id::parse("synthetic-user").unwrap(),
            timezone: zone.map(str::to_owned),
        };
        assert!(matches!(
            AccountCalendar::from_profile(&profile),
            Err(Error::TimezoneUnavailable)
        ));
    }
}

#[test]
fn account_calendar_uses_iana_dst_rules_instead_of_a_fixed_offset() {
    let calendar = calendar(Some("America/New_York"));
    assert_eq!(calendar.zone_name(), "America/New_York");
    assert_eq!(
        calendar
            .today_at("2026-01-15T04:30:00Z".parse().unwrap())
            .unwrap(),
        day("2026-01-14")
    );
    assert_eq!(
        calendar
            .today_at("2026-07-15T04:30:00Z".parse().unwrap())
            .unwrap(),
        day("2026-07-15")
    );
    let groups = calendar
        .schedule(&[], "2026-07-15T04:30:00Z".parse().unwrap(), 2)
        .unwrap();
    assert!(groups[0].kind == DayKind::Date(day("2026-07-15")));
}

#[test]
fn calendar_and_floating_dates_keep_their_day_while_instants_follow_the_account() {
    let calendar = calendar(Some("Pacific/Auckland"));
    assert_eq!(
        calendar.due_day(&due("2026-10-16")).unwrap(),
        day("2026-10-16")
    );
    assert_eq!(
        calendar.due_day(&due("2026-10-16T23:30:00")).unwrap(),
        day("2026-10-16")
    );
    assert_eq!(
        calendar.due_day(&due("2026-10-16T13:30:00Z")).unwrap(),
        day("2026-10-17")
    );
}

#[test]
fn repeated_autumn_hour_keeps_instant_and_floating_overdue_semantics_distinct() {
    let calendar = calendar(Some("America/New_York"));
    let now = "2026-11-01T06:15:00Z".parse().unwrap();
    assert!(
        calendar
            .due_is_overdue_at(&due("2026-11-01T05:30:00Z"), now)
            .unwrap()
    );
    assert!(
        !calendar
            .due_is_overdue_at(&due("2026-11-01T01:30:00"), now)
            .unwrap()
    );
}

#[test]
fn a_named_zone_without_an_instant_cannot_silently_become_a_floating_due_time() {
    let raw = serde_json::from_value(
        json!({"date":"2026-10-16T12:00:00", "timezone":"America/New_York"}),
    )
    .unwrap();
    assert!(matches!(DueDate::parse(raw), Err(Error::InvalidDate)));
    let legacy = serde_json::from_value(json!({"date":"2026-10-16", "datetime":"2026-10-16T16:00:00Z", "timezone":"America/New_York"})).unwrap();
    assert!(DueDate::parse(legacy).is_ok());
}

#[test]
fn manitoba_keeps_its_daylight_offset_after_the_2026_rule_change() {
    // IANA 2026e: America/Winnipeg remains at -05 after November 1.
    let calendar = calendar(Some("America/Winnipeg"));
    let instant = "2026-11-02T05:30:00Z";
    assert_eq!(
        calendar.today_at(instant.parse().unwrap()).unwrap(),
        day("2026-11-02")
    );
    assert_eq!(calendar.due_day(&due(instant)).unwrap(), day("2026-11-02"));
}

#[test]
fn future_instant_grouping_uses_its_own_dst_offset_instead_of_todays_offset() {
    let calendar = calendar(Some("America/New_York"));
    let task = Task {
        id: Id::parse("synthetic-task").unwrap(),
        content: "Private task".into(),
        description: String::new(),
        project_id: Id::parse("project").unwrap(),
        parent_id: None,
        priority: crate::todoist::Priority::from_api(1).unwrap(),
        due: Some(due("2026-11-02T04:30:00Z")),
        labels: Vec::new(),
        completed: false,
        deleted: false,
        created_at: None,
        order: 0,
    };
    let groups = calendar
        .schedule(&[task], "2026-10-31T12:00:00Z".parse().unwrap(), 4)
        .unwrap();
    let november_first = groups
        .iter()
        .find(|group| group.kind == DayKind::Date(day("2026-11-01")))
        .unwrap();
    let november_second = groups
        .iter()
        .find(|group| group.kind == DayKind::Date(day("2026-11-02")))
        .unwrap();
    assert_eq!(november_first.tasks.len(), 1);
    assert!(november_second.tasks.is_empty());
}

#[test]
fn unsupported_instants_return_errors_without_panics_or_a_fabricated_offset() {
    use chrono::TimeZone as _;
    let calendar = calendar(Some("UTC"));
    let future = Utc.with_ymd_and_hms(20000, 1, 1, 0, 0, 0).single().unwrap();
    assert_eq!(calendar.today_at(future), Err(Error::InvalidDate));
    assert!(matches!(
        calendar.schedule(&[], future, 2),
        Err(Error::InvalidDate)
    ));
    assert_eq!(
        calendar
            .today_at("2026-10-16T00:00:00Z".parse().unwrap())
            .unwrap(),
        day("2026-10-16")
    );
}
