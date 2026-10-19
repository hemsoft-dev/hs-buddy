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
