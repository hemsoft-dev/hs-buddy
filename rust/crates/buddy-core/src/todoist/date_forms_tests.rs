//! The exact due-date text Todoist may send, and what each form means.
use serde_json::json;

use super::*;
use crate::todoist::{AccountCalendar, Id, Profile, parse_tasks};

fn due(text: &str) -> Result<DueDate, Error> {
    let raw: RawDue = serde_json::from_value(json!({"date": text})).unwrap();
    DueDate::parse(raw)
}
fn utc() -> AccountCalendar {
    AccountCalendar::from_profile(&Profile {
        id: Id::parse("synthetic-user").unwrap(),
        timezone: Some("UTC".into()),
    })
    .unwrap()
}
fn form(text: &str) -> &'static str {
    match due(text).map(|due| due.kind) {
        Ok(DueKind::Calendar(_)) => "calendar",
        Ok(DueKind::Floating(_)) => "floating",
        Ok(DueKind::Instant(_)) => "instant",
        Err(_) => "invalid",
    }
}
/// A due time is overdue only after it has passed, so the pair of clocks one
/// nanosecond apart pins the parsed value exactly.
fn assert_due_at(text: &str, at: &str, just_after: &str) {
    let due = due(text).unwrap_or_else(|_| panic!("{text} was rejected"));
    let calendar = utc();
    assert!(
        !calendar
            .due_is_overdue_at(&due, at.parse().unwrap())
            .unwrap(),
        "{text} is before {at}"
    );
    assert!(
        calendar
            .due_is_overdue_at(&due, just_after.parse().unwrap())
            .unwrap(),
        "{text} is after {at}"
    );
}

#[test]
fn calendar_dates_need_the_exact_ten_character_form() {
    for (text, day) in [
        ("2026-10-16", "2026-10-16"),
        ("2028-02-29", "2028-02-29"),
        ("0001-01-01", "0001-01-01"),
        ("9999-12-31", "9999-12-31"),
    ] {
        assert_eq!(form(text), "calendar", "{text}");
        let parsed = due(text).unwrap();
        assert_eq!(utc().due_day(&parsed).unwrap().to_string(), day);
    }
    for text in [
        "2026-02-30",
        "2027-02-29",
        "0000-01-01",
        "2026-13-01",
        "2026-00-10",
        "2026-10-00",
        "2026-10-32",
        "2026-1-016",
        "2026-10-1",
        "20261016",
        "2026/10/16",
        " 2026-10-16",
        "2026-10-16 ",
        "+2026-10-16",
        "２０２６-10-16",
        "",
    ] {
        assert_eq!(form(text), "invalid", "{text:?}");
    }
}

#[test]
fn floating_times_allow_unpadded_fields_and_keep_nine_fraction_digits() {
    for (text, at, just_after) in [
        (
            "2026-10-16T12:00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16T12:00:00.5",
            "2026-10-16T12:00:00.5Z",
            "2026-10-16T12:00:00.500000001Z",
        ),
        (
            "2026-10-16T12:00:00.123456789987",
            "2026-10-16T12:00:00.123456789Z",
            "2026-10-16T12:00:00.12345679Z",
        ),
        (
            "2026-1-6T1:2:3",
            "2026-01-06T01:02:03Z",
            "2026-01-06T01:02:03.000000001Z",
        ),
        (
            " 2026-10-16T12:00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026- 10- 16T 12: 00: 00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "+2026-10-16T12:00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "9999-12-30T21:59:59.999999998",
            "9999-12-30T21:59:59.999999998Z",
            "9999-12-30T21:59:59.999999999Z",
        ),
    ] {
        assert_eq!(form(text), "floating", "{text:?}");
        assert_due_at(text, at, just_after);
    }
    let parsed = due("2026-1-6T23:59:59").unwrap();
    assert_eq!(utc().due_day(&parsed).unwrap().to_string(), "2026-01-06");
    for text in [
        "2026-10-16T12:00",
        "2026-10-16T12",
        "2026-10-16T12:00:00.",
        "2026-10-16T12:00:00,5",
        "2026-10-16t12:00:00",
        "2026-10-16 12:00:00",
        "2026-10-16T24:00:00",
        "2026-10-16T12:60:00",
        "2026-10-16T12:00:61",
        "2026-10-16T12:00:00 ",
        "2026-10-16T12:00:00 .5",
        "2026-02-30T12:00:00",
        "0000-10-16T12:00:00",
        "+12026-10-16T12:00:00",
        "-2026-10-16T12:00:00",
        "20261016T120000",
    ] {
        assert_eq!(form(text), "invalid", "{text:?}");
    }
}

#[test]
fn instants_follow_rfc_3339_with_a_colon_in_the_offset() {
    for (text, at, just_after) in [
        (
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16t12:00:00z",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16 12:00:00Z",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16T12:00:00+00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16T12:00:00-00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00.000000001Z",
        ),
        (
            "2026-10-16T12:00:00.5+05:30",
            "2026-10-16T06:30:00.5Z",
            "2026-10-16T06:30:00.500000001Z",
        ),
        (
            "2026-10-16T12:00:00.123456789987-23:59",
            "2026-10-17T11:59:00.123456789Z",
            "2026-10-17T11:59:00.12345679Z",
        ),
        (
            "2026-10-16T12:00:00\u{2212}05:00",
            "2026-10-16T17:00:00Z",
            "2026-10-16T17:00:00.000000001Z",
        ),
        (
            "2026-10-16T12:00:00+23:59",
            "2026-10-15T12:01:00Z",
            "2026-10-15T12:01:00.000000001Z",
        ),
        (
            "0001-01-01T00:00:00+14:00",
            "0000-12-31T10:00:00Z",
            "0000-12-31T10:00:00.000000001Z",
        ),
        (
            "9999-12-30T21:59:59Z",
            "9999-12-30T21:59:59Z",
            "9999-12-30T21:59:59.000000001Z",
        ),
    ] {
        assert_eq!(form(text), "instant", "{text:?}");
        assert_due_at(text, at, just_after);
    }
    for text in [
        "2026-10-16T12:00:00+0530",
        "2026-10-16T12:00:00+05",
        "2026-10-16T12:00:00+24:00",
        "2026-10-16T12:00:00+05:60",
        "2026-10-16T12:00:00+5:30",
        "2026-10-16T12:00:00 Z",
        "2026-10-16T12:00:00Z ",
        "2026-10-16T12:00:00+05:30 ",
        "2026-10-16T12:00:00UTC",
        "2026-10-16T12:00:00Z[UTC]",
        "2026-10-16T12:00:00+00:00[Europe/London]",
        "2026-10-16T12:00:00,5Z",
        "2026-10-16T12:00:00.Z",
        "2026-10-16T12:00Z",
        "2026-10-16T12Z",
        "20261016T120000Z",
        "2026-10-16T24:00:00Z",
        "2026-10-16T12:00:61Z",
        "2026-02-30T12:00:00Z",
        "0000-01-01T00:00:00Z",
        "+002026-10-16T12:00:00Z",
        "2026-1-16T12:00:00Z",
    ] {
        assert_eq!(form(text), "invalid", "{text:?}");
    }
}

#[test]
fn an_instant_in_the_legacy_datetime_field_must_equal_the_date_field_instant() {
    let parse = |value| DueDate::parse(serde_json::from_value::<RawDue>(value).unwrap());
    assert!(
        parse(json!({"date":"2026-10-16T12:00:00Z","datetime":"2026-10-16T08:00:00-04:00"}))
            .is_ok()
    );
    assert!(
        parse(json!({"date":"2026-10-16T12:00:00Z","datetime":"2026-10-16T12:00:00.000000001Z"}))
            .is_err()
    );
    assert!(parse(json!({"date":"2026-10-16","datetime":"2026-10-16T12:00:00"})).is_err());
    assert!(parse(json!({"date":"2026-10-16","datetime":"2026-10-16"})).is_err());
}

#[test]
fn creation_times_are_rfc_3339_without_a_year_limit() {
    let accepts = |added_at: &str| {
        parse_tasks(
            &serde_json::to_vec(&json!({"next_cursor":null,"results":[{
                "id":"task","content":"Synthetic","description":"","project_id":"project",
                "priority":1,"due":null,"labels":[],"checked":false,"child_order":0,
                "added_at":added_at}]}))
            .unwrap(),
        )
        .is_ok()
    };
    for text in [
        "2026-10-16T10:00:00Z",
        "2026-10-16 10:00:00.123456+02:00",
        "2026-10-16t10:00:00.123456789123z",
        "0000-01-01T00:00:00Z",
    ] {
        assert!(accepts(text), "{text:?}");
    }
    for text in [
        "",
        "2026-10-16",
        "2026-10-16T10:00:00",
        "2026-10-16T10:00Z",
        "2026-10-16T10:00:00+0200",
        "2026-10-16T10:00:00Z[UTC]",
        "1792144800",
    ] {
        assert!(!accepts(text), "{text:?}");
    }
}

#[test]
fn a_leap_second_due_time_reads_as_the_second_before_it() {
    // chrono kept a 60th second, and such an instant then failed every
    // calendar view with an invalid date. jiff has no such value and reads it
    // as second 59, so the task is placed.
    for (text, at, just_after, day) in [
        (
            "2016-12-31T23:59:60Z",
            "2016-12-31T23:59:59Z",
            "2016-12-31T23:59:59.000000001Z",
            "2016-12-31",
        ),
        (
            "2026-10-16T12:30:60.5+01:00",
            "2026-10-16T11:30:59.5Z",
            "2026-10-16T11:30:59.500000001Z",
            "2026-10-16",
        ),
    ] {
        assert_eq!(form(text), "instant", "{text:?}");
        assert_due_at(text, at, just_after);
        let parsed = due(text).unwrap();
        assert_eq!(utc().due_day(&parsed).unwrap().to_string(), day);
    }
    assert_eq!(form("2026-10-16T12:00:60"), "floating");
    assert_due_at(
        "2026-10-16T12:00:60",
        "2026-10-16T12:00:59Z",
        "2026-10-16T12:00:59.000000001Z",
    );
}

#[test]
fn instants_end_at_the_last_one_jiff_holds() {
    assert_eq!(form("9999-12-30T22:00:00.999999999Z"), "instant");
    // chrono read these. They are past jiff's last instant.
    for text in [
        "9999-12-30T22:00:01Z",
        "9999-12-31T23:59:59Z",
        "9999-12-31T23:59:59.999999999-23:59",
    ] {
        assert_eq!(form(text), "invalid", "{text:?}");
    }
}
