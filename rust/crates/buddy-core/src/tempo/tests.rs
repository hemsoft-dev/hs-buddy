use chrono::{NaiveDate, Timelike};
use serde_json::{Value, json};

use super::*;

fn row(id: u64, date: &str, seconds: u64) -> Value {
    json!({
        "tempoWorklogId":id,"issue":{"id":124,"self":"https://api.tempo.io/4/issues/124"},
        "author":{"accountId":"123456:synthetic-user"},"timeSpentSeconds":seconds,"startDate":date,
        "createdAt":"2026-10-16T01:00:00Z","updatedAt":"2026-10-16T02:00:00Z",
        "attributes":{"self":"https://api.tempo.io/4/worklogs/1/work-attribute-values","values":[]},
        "self":"https://api.tempo.io/4/worklogs/1"
    })
}
fn page(rows: Vec<Value>) -> Value {
    json!({"metadata":{"count":rows.len(),"limit":50,"offset":0},"results":rows,"self":"https://api.tempo.io/4/worklogs/user/123456:synthetic-user"})
}
fn parse(value: Value) -> Result<Page, Error> {
    parse_worklogs(&serde_json::to_vec(&value).unwrap())
}
fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}
fn author() -> AccountId {
    AccountId::parse("123456:synthetic-user").unwrap()
}

#[test]
fn summary_keeps_unavailable_schedule_distinct_from_an_explicit_zero_hour_holiday() {
    let range = DateRange::new(date("2026-10-16"), date("2026-10-17")).unwrap();
    let rows = parse(page(vec![row(1, "2026-10-16", 3601)])).unwrap().rows;
    let holiday = DaySchedule {
        date: date("2026-10-17"),
        required_seconds: 0,
        kind: ScheduleKind::Holiday,
        holiday_name: Some("Synthetic holiday".into()),
    };
    let partial = summarize(&rows, &author(), range, std::slice::from_ref(&holiday)).unwrap();
    assert_eq!(partial.logged_seconds, 3601);
    assert!(partial.required_seconds.is_none());
    assert!(partial.days[0].balance_seconds().is_none());
    assert_eq!(partial.days[1].balance_seconds(), Some(0));
    assert!(
        partial.days[1]
            .schedule
            .as_ref()
            .unwrap()
            .holiday_name
            .as_deref()
            == Some("Synthetic holiday")
    );
    let working = DaySchedule {
        date: date("2026-10-16"),
        required_seconds: 28800,
        kind: ScheduleKind::Working,
        holiday_name: None,
    };
    let complete = summarize(&rows, &author(), range, &[holiday, working]).unwrap();
    assert_eq!(complete.required_seconds, Some(28800));
    assert_eq!(complete.days[0].balance_seconds(), Some(-25199));
    assert!(complete.days[0].logged.worklogs[0].id.get() == 1);
}

#[test]
fn summary_rejects_duplicate_or_out_of_range_schedule_instead_of_truncating_it() {
    let range = DateRange::new(date("2026-10-16"), date("2026-10-17")).unwrap();
    let day = DaySchedule {
        date: range.start,
        required_seconds: 1,
        kind: ScheduleKind::Working,
        holiday_name: None,
    };
    assert!(matches!(
        summarize(&[], &author(), range, &[day.clone(), day.clone()]),
        Err(Error::InvalidResponse)
    ));
    let outside = DaySchedule {
        date: date("2026-10-18"),
        ..day
    };
    assert!(matches!(
        summarize(&[], &author(), range, &[outside]),
        Err(Error::WrongScope)
    ));
}

#[test]
fn summary_rejects_cross_day_total_overflow_and_wrong_user_without_losing_seconds() {
    let range = DateRange::new(date("2026-10-16"), date("2026-10-18")).unwrap();
    let rows = parse(page(vec![
        row(1, "2026-10-16", i64::MAX as u64),
        row(2, "2026-10-17", i64::MAX as u64),
        row(3, "2026-10-18", i64::MAX as u64),
    ]))
    .unwrap()
    .rows;
    assert!(matches!(
        summarize(&rows, &author(), range, &[]),
        Err(Error::TotalOverflow)
    ));
    let another = AccountId::parse("another-user").unwrap();
    assert!(matches!(
        summarize(&rows[..1], &another, range, &[]),
        Err(Error::WrongScope)
    ));
}

#[test]
fn optional_time_and_issue_id_do_not_acquire_invented_values() {
    let mut value = row(1, "2026-10-16", 3601);
    value["issue"].as_object_mut().unwrap().remove("id");
    let records = parse(page(vec![value])).unwrap().rows;
    assert_eq!(records[0].seconds, 3601);
    assert!(records[0].issue.is_none());
    assert!(records[0].start_time.is_none());
    assert!(records[0].start_instant.is_none());
    assert!(records[0].description.is_none());
    assert!(records[0].author.as_str() == "123456:synthetic-user");
}

#[test]
fn utc_start_and_local_workday_are_preserved_independently_across_midnight() {
    let mut value = row(1, "2026-10-16", 5400);
    value["startTime"] = json!("23:30:00");
    value["startDateTimeUtc"] = json!("2026-10-17T03:30:00Z");
    let rows = parse(page(vec![value])).unwrap().rows;
    assert_eq!(rows[0].start_time.unwrap().hour(), 23);
    assert_eq!(
        rows[0].start_instant.unwrap().date_naive(),
        date("2026-10-17")
    );
    let days = day_totals(
        &rows,
        &author(),
        DateRange::new(date("2026-10-16"), date("2026-10-17")).unwrap(),
    )
    .unwrap();
    assert_eq!(days[0].seconds, 5400);
    assert_eq!(days[1].seconds, 0);
}

#[test]
fn unexpanded_billing_attributes_are_distinct_from_an_observed_empty_list() {
    let mut value = row(1, "2026-10-16", 1);
    value["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("values");
    let rows = parse(page(vec![value.clone()])).unwrap().rows;
    assert!(matches!(
        rows[0].billing_account(),
        Err(Error::AttributesUnavailable)
    ));
    value["attributes"]["values"] = json!([]);
    assert!(matches!(
        parse(page(vec![value.clone()])).unwrap().rows[0].billing_account(),
        Ok(None)
    ));
    value["attributes"]["values"] =
        json!([{"key":"_Account_","value":"SYNTHETIC"},{"key":"_Optional_"}]);
    assert!(matches!(
        parse(page(vec![value.clone()])).unwrap().rows[0].billing_account(),
        Ok(Some("SYNTHETIC"))
    ));
    value["attributes"]["values"] =
        json!([{"key":"_Account_","value":"one"},{"key":"_Account_","value":"two"}]);
    assert!(matches!(
        parse(page(vec![value])),
        Err(Error::InvalidResponse)
    ));
}

#[test]
fn invalid_or_missing_required_data_rejects_the_entire_page_without_private_diagnostics() {
    for (key, invalid) in [
        ("tempoWorklogId", json!(0)),
        ("timeSpentSeconds", json!(-1)),
        ("timeSpentSeconds", json!(1.5)),
        ("startDate", json!("2026-02-30")),
        ("startTime", json!("25:00:00")),
        ("startDateTimeUtc", json!("2026-10-16T09:00:00")),
        ("createdAt", json!("invalid")),
    ] {
        let mut value = row(2, "2026-10-16", 1);
        value[key] = invalid;
        value["description"] = json!("private worklog diagnostic sentinel");
        let error = match parse(page(vec![row(1, "2026-10-16", 1), value])) {
            Err(error) => error,
            Ok(_) => panic!("malformed page must not partially succeed"),
        };
        assert_eq!(error, Error::InvalidResponse);
        assert!(
            !error
                .to_string()
                .contains("private worklog diagnostic sentinel")
        );
    }
    for required in [
        "author",
        "issue",
        "createdAt",
        "updatedAt",
        "attributes",
        "self",
    ] {
        let mut value = row(1, "2026-10-16", 1);
        value.as_object_mut().unwrap().remove(required);
        assert!(matches!(
            parse(page(vec![value])),
            Err(Error::InvalidResponse)
        ));
    }
}

#[test]
fn page_metadata_counts_limits_offsets_and_duplicates_are_not_silent_truncation() {
    let mut value = page(vec![row(1, "2026-10-16", 1)]);
    value["metadata"]["next"] =
        json!("https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=50");
    let parsed = parse(value.clone()).unwrap();
    assert!(parsed.next.is_some());
    assert_eq!(parsed.offset, 0);
    assert_eq!(parsed.limit, 50);
    value["metadata"]["count"] = json!(2);
    assert!(matches!(parse(value), Err(Error::InvalidResponse)));
    let mut value = page(vec![]);
    value["metadata"].as_object_mut().unwrap().remove("offset");
    assert!(matches!(parse(value), Err(Error::InvalidResponse)));
    assert!(matches!(
        parse(page(vec![row(1, "2026-10-16", 1), row(1, "2026-10-16", 1)])),
        Err(Error::InvalidResponse)
    ));
    assert!(matches!(
        parse_worklogs(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Err(Error::TooLarge)
    ));
}

#[test]
fn durations_over_a_day_remain_exact_and_integer_totals_cannot_overflow() {
    let rows = parse(page(vec![
        row(1, "2026-10-16", 90001),
        row(2, "2026-10-16", 1),
    ]))
    .unwrap()
    .rows;
    let range = DateRange::new(date("2026-10-16"), date("2026-10-16")).unwrap();
    assert_eq!(
        day_totals(&rows, &author(), range).unwrap()[0].seconds,
        90002
    );
    let rows = parse(page(
        (1..=3)
            .map(|id| row(id, "2026-10-16", i64::MAX as u64))
            .collect(),
    ))
    .unwrap()
    .rows;
    assert!(matches!(
        day_totals(&rows, &author(), range),
        Err(Error::TotalOverflow)
    ));
}

#[test]
fn scope_mismatch_and_cross_page_duplicates_cannot_replace_selected_users_totals() {
    let rows = parse(page(vec![row(1, "2026-10-16", 1)])).unwrap().rows;
    let range = DateRange::new(date("2026-10-16"), date("2026-10-16")).unwrap();
    assert!(matches!(
        day_totals(&rows, &AccountId::parse("other-user").unwrap(), range),
        Err(Error::WrongScope)
    ));
    let other_range = DateRange::new(date("2026-10-17"), date("2026-10-17")).unwrap();
    assert!(matches!(
        day_totals(&rows, &author(), other_range),
        Err(Error::WrongScope)
    ));
    let mut twice = rows.clone();
    twice.extend(rows);
    assert!(matches!(
        day_totals(&twice, &author(), range),
        Err(Error::InvalidResponse)
    ));
}

#[test]
fn inclusive_weeks_keep_empty_days_and_cross_leap_boundaries_without_clock_arithmetic() {
    let week = DateRange::week_containing(date("2028-03-01")).unwrap();
    assert_eq!(week.start, date("2028-02-28"));
    assert_eq!(week.end, date("2028-03-05"));
    let days = day_totals(&[], &author(), week).unwrap();
    assert_eq!(days.len(), 7);
    assert_eq!(days[1].date, date("2028-02-29"));
    assert!(
        days.iter()
            .all(|day| day.seconds == 0 && day.worklogs.is_empty())
    );
    assert!(matches!(
        DateRange::new(week.end, week.start),
        Err(Error::InvalidRange)
    ));
    assert!(matches!(
        DateRange::new(date("2026-01-01"), date("2027-01-02")),
        Err(Error::InvalidRange)
    ));
}

#[test]
fn user_schedules_distinguish_holidays_nonworking_and_missing_holiday_names() {
    let value = json!({"self":"https://api.tempo.io/4/user-schedule","metadata":{"count":3},"results":[
        {"date":"2026-10-16","requiredSeconds":28800,"type":"WORKING_DAY"},
        {"date":"2026-10-17","requiredSeconds":0,"type":"NON_WORKING_DAY"},
        {"date":"2026-10-18","requiredSeconds":0,"type":"HOLIDAY","holiday":{"name":"Synthetic holiday"}}
    ]});
    let parse = |value: &Value| parse_schedule(&serde_json::to_vec(value).unwrap());
    let days = parse(&value).unwrap();
    assert!(days[0].kind == ScheduleKind::Working);
    assert_eq!(days[0].required_seconds, 28800);
    assert!(days[1].kind == ScheduleKind::NonWorking);
    assert_eq!(days[2].holiday_name.as_deref(), Some("Synthetic holiday"));
    let mut invalid = value.clone();
    invalid["results"][1]["date"] = json!("2026-10-16");
    assert!(matches!(parse(&invalid), Err(Error::InvalidResponse)));
    let mut invalid = value;
    invalid["results"][0]["type"] = json!("UNKNOWN");
    assert!(matches!(parse(&invalid), Err(Error::InvalidResponse)));
}
