//! Expected times come from `enumerateCronOccurrences` in
//! `shared/utils/cronUtils.ts` on cron-parser 5.10.2 with the same arguments.
use super::*;

fn ms(milliseconds: i64) -> Timestamp {
    Timestamp::from_millisecond(milliseconds).unwrap()
}

/// 2026-11-28T12:00:00Z, a Saturday.
const SATURDAY_NOON: i64 = 1_795_867_200_000;

fn times(expression: &str, zone: &str, from: i64, to: i64, limit: usize) -> Option<Vec<String>> {
    let cron = Cron::parse(expression).ok()?;
    let zone = TimeZone::get(zone).unwrap();
    Some(
        cron.occurrences(&zone, ms(from), ms(to), limit)
            .into_iter()
            .map(|time| time.to_string())
            .collect(),
    )
}

#[test]
fn weekday_mornings_read_in_the_schedule_zone() {
    assert_eq!(
        times(
            "0 9 * * 1-5",
            "America/New_York",
            SATURDAY_NOON,
            1_796_472_000_000,
            500
        )
        .unwrap(),
        [
            "2026-11-30T14:00:00Z",
            "2026-12-01T14:00:00Z",
            "2026-12-02T14:00:00Z",
            "2026-12-03T14:00:00Z",
            "2026-12-04T14:00:00Z",
        ]
    );
}

#[test]
fn a_time_in_the_spring_gap_runs_after_it_and_once() {
    // 02:30 does not exist on 2026-03-08 in New York; it runs at 03:30 EDT.
    assert_eq!(
        times(
            "30 2 * * *",
            "America/New_York",
            1_772_946_000_000,
            1_773_205_200_000,
            500
        )
        .unwrap(),
        [
            "2026-03-08T07:30:00Z",
            "2026-03-09T06:30:00Z",
            "2026-03-10T06:30:00Z"
        ]
    );
    // 02:00 and 02:30 land on 03:00 and 03:30, which are also scheduled: once each.
    assert_eq!(
        times(
            "*/30 1-3 * * *",
            "America/New_York",
            1_772_946_000_000,
            1_773_100_000_000,
            500
        )
        .unwrap(),
        [
            "2026-03-08T06:00:00Z",
            "2026-03-08T06:30:00Z",
            "2026-03-08T07:00:00Z",
            "2026-03-08T07:30:00Z",
            "2026-03-09T05:00:00Z",
            "2026-03-09T05:30:00Z",
            "2026-03-09T06:00:00Z",
            "2026-03-09T06:30:00Z",
            "2026-03-09T07:00:00Z",
            "2026-03-09T07:30:00Z",
        ]
    );
}

#[test]
fn a_repeated_autumn_hour_runs_once_at_its_first_instant() {
    assert_eq!(
        times(
            "*/20 1 * * *",
            "America/New_York",
            1_793_433_600_000,
            1_793_520_000_000,
            500
        )
        .unwrap(),
        [
            "2026-11-01T05:00:00Z",
            "2026-11-01T05:20:00Z",
            "2026-11-01T05:40:00Z"
        ]
    );
}

#[test]
fn every_hour_runs_in_both_passes_of_a_whole_repeated_hour() {
    // 2026-11-01T04:00Z to 07:30Z: 01:00 to 01:59 New York time happens twice.
    let (from, to) = (1_793_505_600_000, 1_793_518_200_000);
    assert_eq!(
        times("*/30 */1 * * *", "America/New_York", from, to, 500).unwrap(),
        [
            "2026-11-01T04:30:00Z",
            "2026-11-01T05:00:00Z",
            "2026-11-01T05:30:00Z",
            "2026-11-01T06:00:00Z",
            "2026-11-01T06:30:00Z",
            "2026-11-01T07:00:00Z",
            "2026-11-01T07:30:00Z",
        ]
    );
    // One hour missing from the field: the repeated hour runs once.
    assert_eq!(
        times("30 0-22 * * *", "America/New_York", from, to, 500).unwrap(),
        [
            "2026-11-01T04:30:00Z",
            "2026-11-01T05:30:00Z",
            "2026-11-01T07:30:00Z"
        ]
    );
    // Lord Howe goes back 30 minutes; cron-parser runs that half hour once.
    assert_eq!(
        times(
            "*/15 * * * *",
            "Australia/Lord_Howe",
            1_775_311_200_000,
            1_775_318_400_000,
            500
        )
        .unwrap(),
        [
            "2026-04-04T14:15:00Z",
            "2026-04-04T14:30:00Z",
            "2026-04-04T14:45:00Z",
            "2026-04-04T15:30:00Z",
            "2026-04-04T15:45:00Z",
            "2026-04-04T16:00:00Z",
        ]
    );
}

#[test]
fn two_restricted_day_fields_match_either_day() {
    assert_eq!(
        times("0 0 13 * 5", "UTC", SATURDAY_NOON, 1_830_000_000_000, 6).unwrap(),
        [
            "2026-12-04T00:00:00Z",
            "2026-12-11T00:00:00Z",
            "2026-12-13T00:00:00Z",
            "2026-12-18T00:00:00Z",
            "2026-12-25T00:00:00Z",
            "2027-01-01T00:00:00Z",
        ]
    );
    // `?` is a wildcard, so only the weekday restricts.
    assert_eq!(
        times("0 0 ? * MON", "UTC", SATURDAY_NOON, 1_797_000_000_000, 500).unwrap(),
        ["2026-11-30T00:00:00Z", "2026-12-07T00:00:00Z"]
    );
}

#[test]
fn names_steps_and_sunday_as_seven_follow_cron_parser() {
    assert_eq!(
        times(
            "0 12 * jan-mar mon",
            "UTC",
            SATURDAY_NOON,
            1_830_000_000_000,
            4
        )
        .unwrap(),
        [
            "2027-01-04T12:00:00Z",
            "2027-01-11T12:00:00Z",
            "2027-01-18T12:00:00Z",
            "2027-01-25T12:00:00Z",
        ]
    );
    assert_eq!(
        times("5/20 * * * *", "UTC", SATURDAY_NOON, 1_795_874_400_000, 500).unwrap(),
        [
            "2026-11-28T12:05:00Z",
            "2026-11-28T12:25:00Z",
            "2026-11-28T12:45:00Z",
            "2026-11-28T13:05:00Z",
            "2026-11-28T13:25:00Z",
            "2026-11-28T13:45:00Z",
        ]
    );
    for sunday in ["0 0 * * 7", "@weekly", "0 0 * * sun"] {
        assert_eq!(
            times(sunday, "UTC", SATURDAY_NOON, 1_797_000_000_000, 500).unwrap(),
            ["2026-11-29T00:00:00Z", "2026-12-06T00:00:00Z"],
            "{sunday}"
        );
    }
    assert_eq!(
        times("0 0 * * 5-7", "UTC", SATURDAY_NOON, 1_796_500_000_000, 500).unwrap(),
        [
            "2026-11-29T00:00:00Z",
            "2026-12-04T00:00:00Z",
            "2026-12-05T00:00:00Z"
        ]
    );
    // cron-parser resolves month names in the weekday field: JUL is 7, Sunday.
    assert_eq!(
        times("0 0 * * jul", "UTC", SATURDAY_NOON, 1_796_500_000_000, 500).unwrap(),
        ["2026-11-29T00:00:00Z"]
    );
    assert_eq!(
        times("0 0 * * Thu", "UTC", SATURDAY_NOON, 1_797_000_000_000, 5).unwrap(),
        ["2026-12-03T00:00:00Z", "2026-12-10T00:00:00Z"]
    );
}

#[test]
fn the_window_excludes_its_start_includes_its_end_and_honors_the_limit() {
    assert_eq!(
        times("* * * * *", "UTC", SATURDAY_NOON, 1_795_867_500_000, 3).unwrap(),
        [
            "2026-11-28T12:01:00Z",
            "2026-11-28T12:02:00Z",
            "2026-11-28T12:03:00Z"
        ]
    );
    let all = times(
        "* * * * *",
        "UTC",
        SATURDAY_NOON,
        SATURDAY_NOON + 120_000,
        500,
    )
    .unwrap();
    assert_eq!(all, ["2026-11-28T12:01:00Z", "2026-11-28T12:02:00Z"]);
    assert!(
        times("* * * * *", "UTC", SATURDAY_NOON, SATURDAY_NOON, 500)
            .unwrap()
            .is_empty()
    );
    assert!(
        times(
            "* * * * *",
            "UTC",
            SATURDAY_NOON,
            SATURDAY_NOON + 120_000,
            0
        )
        .unwrap()
        .is_empty()
    );
    // A date that never comes yields nothing rather than looping.
    assert!(
        times("0 0 30 2 *", "UTC", SATURDAY_NOON, 1_830_000_000_000, 500)
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn cron_parser_quirks_electron_lets_through_are_kept() {
    // `*-5` is read as 0-59, and a step reads its leading digits.
    let midnight = 1_795_824_000_000 - 1;
    assert_eq!(
        times("*-5 0 * * *", "UTC", midnight, midnight + 120_001, 500).unwrap(),
        [
            "2026-11-28T00:00:00Z",
            "2026-11-28T00:01:00Z",
            "2026-11-28T00:02:00Z"
        ]
    );
    assert_eq!(
        times("1/2-3 0 * * *", "UTC", midnight, midnight + 300_001, 500).unwrap(),
        [
            "2026-11-28T00:01:00Z",
            "2026-11-28T00:03:00Z",
            "2026-11-28T00:05:00Z"
        ]
    );
}

#[test]
fn expressions_electron_rejects_are_invalid() {
    for expression in [
        "",
        "0 9 * *",
        "0 0 9 * * *",
        "0 0 * thu *",
        "0 0 1-2-3 * *",
        "0 0 L * *",
        "0 0 * * 5L",
        "0 0 * * 5#2",
        "H 0 * * *",
        "60 * * * *",
        "0 24 * * *",
        "0 0 0 * *",
        "0 0 * 13 *",
        "0 0 * * 8",
        "0 0 * * 1,",
        "0 0 * * */0",
        "0 0 * * 5-1",
        "0 0 * * thursday",
        "0 0 * * xyz",
        "1/2/3 * * * *",
        "@reboot",
        "0 0 * * -1",
    ] {
        assert_eq!(Cron::parse(expression), Err(InvalidCron), "{expression:?}");
    }
    assert!(Cron::parse(&format!("0 0 * * {}", "1,".repeat(600) + "1")).is_err());
    // cron-parser rejects a value listed twice, weekday 7 being 0 when alone.
    for repeated in [
        "0 0 * * 1,1",
        "1-3,2 0 * * *",
        "0 0 * * 0,7",
        "0 0 * * 0,5-7",
    ] {
        assert_eq!(Cron::parse(repeated), Err(InvalidCron), "{repeated}");
    }
    for distinct in ["0 0 * * 7,5-6", "0 0 * * 0-7", "0 0 * * 1-7/2"] {
        assert!(Cron::parse(distinct).is_ok(), "{distinct}");
    }
    assert!(Cron::parse("  @Weekdays ").is_ok());
}
