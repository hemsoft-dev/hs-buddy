//! The exact date and time text Tempo may send in a worklog or schedule.
use serde_json::{Value, json};

use super::*;
use crate::tempo::Draft;

fn worklog(change: impl FnOnce(&mut Value)) -> Result<Worklog, Error> {
    let mut value = json!({
        "tempoWorklogId":1,"self":"https://api.tempo.io/4/worklogs/1",
        "issue":{"id":2,"self":"https://api.tempo.io/4/issues/2"},
        "author":{"accountId":"synthetic-user"},"timeSpentSeconds":600,
        "startDate":"2026-10-16","startTime":"08:00:00","startDateTimeUtc":"2026-10-16T12:00:00Z",
        "createdAt":"2026-10-16T01:00:00Z","updatedAt":"2026-10-16T02:00:00Z",
        "attributes":{"self":"https://api.tempo.io/4/worklogs/1/work-attribute-values","values":[]}
    });
    change(&mut value);
    parse_worklog(&serde_json::to_vec(&value).unwrap())
}
fn with(field: &str, text: &str) -> Result<Worklog, Error> {
    worklog(|value| value[field] = json!(text))
}

#[test]
fn work_dates_must_be_the_canonical_ten_characters() {
    for text in ["2026-10-16", "2028-02-29", "0001-01-01", "9999-12-31"] {
        let row = with("startDate", text).unwrap();
        assert_eq!(row.date.to_string(), text);
    }
    for text in [
        "2026-02-30",
        "0000-01-01",
        "2026-1-016",
        "2026-10-6",
        " 2026-10-6",
        "2026-10- 6",
        "+2026-10-6",
        "+02026-1-1",
        "20261016",
        "2026-10-16T00:00:00Z",
        "",
    ] {
        assert!(
            matches!(with("startDate", text), Err(Error::InvalidResponse)),
            "{text:?}"
        );
    }
    let schedule = |text: &str| {
        parse_schedule(
            &serde_json::to_vec(&json!({"self":"https://api.tempo.io/4/user-schedule",
                "metadata":{"count":1},
                "results":[{"date":text,"requiredSeconds":0,"type":"WORKING_DAY"}]}))
            .unwrap(),
        )
    };
    assert_eq!(
        schedule("2026-10-16").unwrap()[0].date.to_string(),
        "2026-10-16"
    );
    for text in ["2026-10-6", "0000-01-01", "2026-02-30", " 2026-10-6"] {
        assert!(
            matches!(schedule(text), Err(Error::InvalidResponse)),
            "{text:?}"
        );
    }
}

#[test]
fn start_times_allow_unpadded_fields_and_keep_nine_fraction_digits() {
    let time = |text: &str| with("startTime", text).map(|row| row.start_time);
    let same = |left: &str, right: &str| time(left).unwrap() == time(right).unwrap();
    for (text, canonical) in [
        ("08:00:00", "08:00:00"),
        ("8:0:0", "08:00:00"),
        (" 8: 0: 0", "08:00:00"),
        ("23:59:59", "23:59:59"),
        ("08:00:00.0", "08:00:00"),
        ("08:00:00.50", "08:00:00.5"),
        ("08:00:00.123456789987", "08:00:00.123456789"),
    ] {
        assert!(same(text, canonical), "{text:?}");
    }
    assert!(!same("08:00:00.5", "08:00:00"));
    assert!(!same("08:00:00.000000001", "08:00:00"));
    assert!(same("08:00:00.0000000001", "08:00:00"));
    for text in [
        "",
        "08:00",
        "0800",
        "080000",
        "08:00:00.",
        "08:00:00,5",
        "08:00:00 ",
        "24:00:00",
        "08:60:00",
        "08:00:61",
        "T08:00:00",
        "08:00:00Z",
        "8 AM",
        "08:00:00.123456789012345678901234",
    ] {
        assert!(
            matches!(time(text), Err(Error::InvalidResponse)),
            "{text:?}"
        );
    }
    // Only a start time with whole seconds can be written back.
    let fractional = with("startTime", "08:00:00.5").unwrap();
    assert!(matches!(
        Draft::delete(&fractional).map(|_| ()),
        Err(Error::InvalidDraft)
    ));
}

#[test]
fn instants_follow_rfc_3339_and_compare_by_moment() {
    let created = |text: &str| with("createdAt", text).map(|row| row.created_at);
    let same = |left: &str, right: &str| created(left).unwrap() == created(right).unwrap();
    for (text, utc) in [
        ("2026-10-16T02:00:00.123Z", "2026-10-16T02:00:00.123Z"),
        ("2026-10-15T22:00:00.123-04:00", "2026-10-16T02:00:00.123Z"),
        ("2026-10-16t02:00:00.123z", "2026-10-16T02:00:00.123Z"),
        ("2026-10-16 02:00:00.123+00:00", "2026-10-16T02:00:00.123Z"),
        (
            "2026-10-16T02:00:00.1230000009Z",
            "2026-10-16T02:00:00.123Z",
        ),
        ("2026-10-16T07:30:00.123+05:30", "2026-10-16T02:00:00.123Z"),
        (
            "2026-10-16T02:00:00.123\u{2212}00:00",
            "2026-10-16T02:00:00.123Z",
        ),
        ("0000-01-01T00:00:00+23:59", "0000-01-01T00:00:00+23:59"),
    ] {
        assert!(same(text, utc), "{text:?}");
    }
    // 64 bytes are read, and the digits past the ninth are dropped.
    assert!(same(
        "2026-10-16T02:00:00.1230000001234567890123456789012345678901234Z",
        "2026-10-16T02:00:00.123Z"
    ));
    assert!(!same(
        "2026-10-16T02:00:00.123Z",
        "2026-10-16T02:00:00.124Z"
    ));
    assert!(!same(
        "2026-10-16T02:00:00Z",
        "2026-10-16T02:00:00.000000001Z"
    ));
    for field in ["createdAt", "updatedAt", "startDateTimeUtc"] {
        for text in [
            "",
            "2026-10-16",
            "2026-10-16T02:00:00",
            "2026-10-16T02:00Z",
            "2026-10-16T02:00:00+0000",
            "2026-10-16T02:00:00+00",
            "2026-10-16T02:00:00+24:00",
            "2026-10-16T02:00:00Z[UTC]",
            "2026-10-16T02:00:00,5Z",
            "2026-10-16T02:00:00.Z",
            "20261016T020000Z",
            "2026-02-30T02:00:00Z",
            "2026-10-16T24:00:00Z",
            "2026-10-16T02:00:61Z",
            "+002026-10-16T02:00:00Z",
            // 65 bytes, one past the length limit.
            "2026-10-16T02:00:00.12345678901234567890123456789012345678901234Z",
        ] {
            assert!(
                matches!(with(field, text), Err(Error::InvalidResponse)),
                "{field} {text:?}"
            );
        }
    }
}

#[test]
fn a_leap_second_reads_as_the_second_before_it() {
    // chrono kept a 60th second as a value of its own, which then could not be
    // written back. jiff has no such value and reads it as second 59.
    let leap = with("startTime", "23:59:60").unwrap();
    let before = with("startTime", "23:59:59").unwrap();
    assert!(leap.start_time == before.start_time);
    assert!(Draft::delete(&leap).is_ok());
    let leap = with("updatedAt", "2016-12-31T23:59:60Z").unwrap();
    let before = with("updatedAt", "2016-12-31T23:59:59Z").unwrap();
    assert!(leap.updated_at == before.updated_at);
    let leap = with("updatedAt", "2016-12-31T23:59:60.5Z").unwrap();
    let before = with("updatedAt", "2016-12-31T23:59:59.5Z").unwrap();
    assert!(leap.updated_at == before.updated_at);
}

#[test]
fn instants_end_at_the_last_one_jiff_holds() {
    assert!(with("updatedAt", "9999-12-30T22:00:00.999999999Z").is_ok());
    assert!(with("updatedAt", "9999-12-31T21:59:00.999999999+23:59").is_ok());
    // chrono read these. They are past jiff's last instant.
    for text in [
        "9999-12-30T22:00:01Z",
        "9999-12-31T23:59:59Z",
        "9999-12-31T23:59:59.999999999-23:59",
    ] {
        assert!(
            matches!(with("updatedAt", text), Err(Error::InvalidResponse)),
            "{text:?}"
        );
    }
}
