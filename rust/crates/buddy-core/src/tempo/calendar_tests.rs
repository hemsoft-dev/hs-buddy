use super::*;
fn instant(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
fn day(value: &str) -> NaiveDate {
    value.parse().unwrap()
}
fn dates(range: DateRange) -> (NaiveDate, NaiveDate) {
    (range.start, range.end)
}

#[test]
fn captured_account_zone_drives_today_and_month_on_opposite_sides_of_new_year() {
    let clock = instant("2026-12-31T12:30:00Z");
    let west = AccountCalendar::from_zone(Some("America/Los_Angeles")).unwrap();
    let east = AccountCalendar::from_zone(Some("Pacific/Kiritimati")).unwrap();
    assert_eq!(west.zone_name(), "America/Los_Angeles");
    assert_eq!(east.zone_name(), "Pacific/Kiritimati");
    assert_eq!(west.today_at(clock).unwrap(), day("2026-12-31"));
    assert_eq!(east.today_at(clock).unwrap(), day("2027-01-01"));
    assert_eq!(
        dates(west.current_month_at(clock).unwrap()),
        (day("2026-12-01"), day("2026-12-31"))
    );
    assert_eq!(
        dates(east.current_month_at(clock).unwrap()),
        (day("2027-01-01"), day("2027-01-31"))
    );
}
#[test]
fn bundled_dst_offsets_change_the_service_midnight_without_host_zone_rules() {
    let calendar = AccountCalendar::from_zone(Some("America/New_York")).unwrap();
    for (before, after, previous, next) in [
        (
            "2026-03-09T03:59:59Z",
            "2026-03-09T04:00:00Z",
            "2026-03-08",
            "2026-03-09",
        ),
        (
            "2026-11-02T04:59:59Z",
            "2026-11-02T05:00:00Z",
            "2026-11-01",
            "2026-11-02",
        ),
    ] {
        assert_eq!(calendar.today_at(instant(before)).unwrap(), day(previous));
        assert_eq!(calendar.today_at(instant(after)).unwrap(), day(next));
    }
}
#[test]
fn absent_or_unknown_account_zone_never_falls_back_to_the_machine() {
    for zone in [
        None,
        Some(""),
        Some("Etc/Unknown"),
        Some("This/Is_Not_A_Zone"),
    ] {
        assert!(matches!(
            AccountCalendar::from_zone(zone),
            Err(Error::TimezoneUnavailable)
        ));
    }
    assert_eq!(
        AccountCalendar::from_zone(Some("UTC"))
            .unwrap()
            .today_at(instant("2026-10-17T00:00:00Z"))
            .unwrap(),
        day("2026-10-17")
    );
}
#[test]
fn unsupported_instants_return_a_calendar_error_without_panicking() {
    let calendar = AccountCalendar::from_zone(Some("UTC")).unwrap();
    assert!(matches!(
        calendar.today_at(DateTime::<Utc>::MAX_UTC),
        Err(Error::InvalidDate)
    ));
    assert!(matches!(
        calendar.today_at(DateTime::<Utc>::MIN_UTC),
        Err(Error::InvalidDate)
    ));
}
#[test]
fn whole_month_navigation_preserves_leap_days_and_year_boundaries() {
    assert_eq!(
        dates(DateRange::month_containing(day("2028-02-29")).unwrap()),
        (day("2028-02-01"), day("2028-02-29"))
    );
    assert_eq!(
        dates(DateRange::month_containing(day("2026-02-28")).unwrap()),
        (day("2026-02-01"), day("2026-02-28"))
    );
    let december = DateRange::month_containing(day("2026-12-31")).unwrap();
    let january = december.shift_month(1).unwrap();
    assert_eq!(dates(january), (day("2027-01-01"), day("2027-01-31")));
    assert_eq!(dates(january.shift_month(-1).unwrap()), dates(december));
    assert_eq!(dates(january.shift_month(0).unwrap()), dates(january));
}
#[test]
fn month_overflows_and_arbitrary_ranges_cannot_silently_change_selection() {
    let first = DateRange::month_containing(day("0001-01-01")).unwrap();
    let last = DateRange::month_containing(day("9999-12-31")).unwrap();
    for (range, delta) in [(first, -1), (last, 1), (first, i32::MIN), (last, i32::MAX)] {
        assert!(matches!(range.shift_month(delta), Err(Error::InvalidRange)));
    }
    let custom = DateRange::new(day("2026-10-02"), day("2026-10-08")).unwrap();
    assert!(matches!(custom.shift_month(1), Err(Error::InvalidRange)));
}

#[test]
fn today_follows_daylight_changes_odd_offsets_and_a_skipped_calendar_day() {
    for (zone, at, today) in [
        ("America/New_York", "2026-03-08T04:59:59Z", "2026-03-07"),
        ("America/New_York", "2026-03-08T05:00:00Z", "2026-03-08"),
        ("America/New_York", "2026-03-09T03:59:59Z", "2026-03-08"),
        ("America/New_York", "2026-03-09T04:00:00Z", "2026-03-09"),
        ("America/New_York", "2026-11-01T03:59:59Z", "2026-10-31"),
        ("America/New_York", "2026-11-01T04:00:00Z", "2026-11-01"),
        ("America/New_York", "2026-11-02T04:59:59Z", "2026-11-01"),
        ("America/New_York", "2026-11-02T05:00:00Z", "2026-11-02"),
        // Midnight itself was skipped: 23:59:59 was followed by 01:00:00.
        ("America/Sao_Paulo", "2018-11-04T02:59:59Z", "2018-11-03"),
        ("America/Sao_Paulo", "2018-11-04T03:00:00Z", "2018-11-04"),
        // Samoa crossed the date line and had no December 30.
        ("Pacific/Apia", "2011-12-30T09:59:59Z", "2011-12-29"),
        ("Pacific/Apia", "2011-12-30T10:00:00Z", "2011-12-31"),
        ("Pacific/Chatham", "2026-06-30T11:14:59Z", "2026-06-30"),
        ("Pacific/Chatham", "2026-06-30T11:15:00Z", "2026-07-01"),
        ("Asia/Kathmandu", "2026-06-30T18:14:59Z", "2026-06-30"),
        ("Asia/Kathmandu", "2026-06-30T18:15:00Z", "2026-07-01"),
        (
            "Pacific/Kiritimati",
            "2026-06-30T09:59:59.999999999Z",
            "2026-06-30",
        ),
        ("Pacific/Kiritimati", "2026-06-30T10:00:00Z", "2026-07-01"),
        ("UTC", "0001-01-01T00:00:00Z", "0001-01-01"),
        ("UTC", "9999-12-30T22:00:00Z", "9999-12-30"),
    ] {
        let calendar = AccountCalendar::from_zone(Some(zone)).unwrap();
        assert_eq!(
            calendar.today_at(instant(at)).unwrap(),
            day(today),
            "{zone} {at}"
        );
    }
}
#[test]
fn ranges_stay_inside_years_1_to_9999_and_weeks_start_on_monday() {
    assert_eq!(
        dates(DateRange::week_containing(day("0001-01-07")).unwrap()),
        (day("0001-01-01"), day("0001-01-07"))
    );
    assert_eq!(
        dates(DateRange::week_containing(day("2026-10-18")).unwrap()),
        (day("2026-10-12"), day("2026-10-18"))
    );
    assert_eq!(
        dates(DateRange::week_containing(day("2026-10-19")).unwrap()),
        (day("2026-10-19"), day("2026-10-25"))
    );
    assert_eq!(
        dates(DateRange::week_containing(day("9999-12-26")).unwrap()),
        (day("9999-12-20"), day("9999-12-26"))
    );
    // 9999-12-27 is a Monday, so its week would end in the year 10000.
    for text in ["9999-12-27", "9999-12-31"] {
        assert!(matches!(
            DateRange::week_containing(day(text)),
            Err(Error::InvalidRange)
        ));
    }
    assert!(DateRange::new(day("2026-01-01"), day("2027-01-01")).is_ok());
    assert!(DateRange::new(day("0001-01-01"), day("0001-12-31")).is_ok());
    assert!(matches!(
        DateRange::new(day("0000-12-31"), day("0001-01-01")),
        Err(Error::InvalidRange)
    ));
    assert_eq!(
        dates(DateRange::month_containing(day("9999-12-15")).unwrap()),
        (day("9999-12-01"), day("9999-12-31"))
    );
    assert_eq!(
        dates(
            DateRange::month_containing(day("2026-03-31"))
                .unwrap()
                .shift_month(-13)
                .unwrap()
        ),
        (day("2025-02-01"), day("2025-02-28"))
    );
}
