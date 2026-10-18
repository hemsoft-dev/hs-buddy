use super::*;
use crate::tempo::jira::tests::{Fixture, reply};
use serde_json::{Value, json};

fn profile() -> &'static Path {
    Path::new(if cfg!(windows) {
        "D:/synthetic-unused/tempo-planner.json"
    } else {
        "/synthetic-unused/tempo-planner.json"
    })
}
fn range(start: &str, end: &str) -> DateRange {
    DateRange::new(start.parse().unwrap(), end.parse().unwrap()).unwrap()
}
fn now() -> DateTime<Utc> {
    "2026-10-16T12:00:00Z".parse().unwrap()
}
fn identity(account: &str, zone: Option<&str>) -> String {
    reply(
        200,
        &json!({"accountId":account,"timeZone":zone}).to_string(),
    )
}
fn user() -> String {
    identity("synthetic-user", Some("America/New_York"))
}
fn worklog(id: u64, date: &str, seconds: u64) -> Value {
    let next_day = date
        .parse::<chrono::NaiveDate>()
        .unwrap()
        .succ_opt()
        .unwrap();
    json!({"tempoWorklogId":id,"self":format!("https://api.tempo.io/4/worklogs/{id}"),"issue":{"id":123,"self":"https://api.tempo.io/4/issues/123"},
        "author":{"accountId":"synthetic-user"},"timeSpentSeconds":seconds,"startDate":date,"startTime":"23:45:00",
        "startDateTimeUtc":format!("{next_day}T03:45:00Z"),"description":"Private synthetic worklog 界",
        "createdAt":"2026-10-16T01:00:00Z","updatedAt":"2026-10-16T02:00:00Z",
        "attributes":{"self":format!("https://api.tempo.io/4/worklogs/{id}/work-attribute-values"),"values":[]}})
}
fn worklogs(rows: Vec<Value>) -> String {
    reply(200,&json!({"self":"https://api.tempo.io/4/worklogs","metadata":{"count":rows.len(),"limit":50,"offset":0},"results":rows}).to_string())
}
fn schedules(days: Vec<(&str, u64)>) -> String {
    let rows:Vec<_>=days.into_iter().map(|(date,seconds)|json!({"date":date,"requiredSeconds":seconds,"type":if seconds==0 {"NON_WORKING_DAY"} else {"WORKING_DAY"}})).collect();
    reply(200,&json!({"self":"https://api.tempo.io/4/user-schedule","metadata":{"count":rows.len()},"results":rows}).to_string())
}
fn complete(seconds: u64) -> Vec<String> {
    complete_range(seconds, "2026-10-16", "2026-10-17")
}
fn complete_range(seconds: u64, start: &str, end: &str) -> Vec<String> {
    vec![
        user(),
        worklogs(vec![worklog(7, start, seconds)]),
        schedules(vec![(start, 28800), (end, 0)]),
        user(),
    ]
}
fn clients(fixture: &Fixture) -> (Arc<Client>, Arc<JiraClient>) {
    (
        Arc::new(
            Client::build(
                "synthetic-tempo-token",
                fixture.base.join("4/").unwrap(),
                false,
            )
            .unwrap(),
        ),
        Arc::new(
            JiraClient::build(
                fixture.base.clone(),
                "synthetic@example.invalid",
                "synthetic-jira-token",
                false,
            )
            .unwrap(),
        ),
    )
}
async fn connect(fixture: &Fixture) -> Planner {
    let (tempo, jira) = clients(fixture);
    Planner::connect(
        profile(),
        tempo,
        jira,
        range("2026-10-16", "2026-10-17"),
        now(),
    )
    .await
    .unwrap()
}
fn all_gets(fixture: &Fixture, expected: usize) {
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), expected);
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
}

#[tokio::test]
async fn initial_current_month_uses_the_captured_account_zone_for_both_service_reads() {
    let zone = Some("Pacific/Kiritimati");
    let fixture = Fixture::new(vec![
        identity("synthetic-user", zone),
        worklogs(vec![]),
        schedules(vec![]),
        identity("synthetic-user", zone),
    ]);
    let (tempo, jira) = clients(&fixture);
    let clock = "2026-12-31T12:30:00Z".parse().unwrap();
    let planner = Planner::connect_current_month_at(profile(), tempo, jira, clock)
        .await
        .unwrap();
    assert_eq!(planner.snapshot().time_zone.as_deref(), zone);
    assert_eq!(
        planner.snapshot().range.start,
        "2027-01-01".parse::<chrono::NaiveDate>().unwrap()
    );
    assert_eq!(
        planner.snapshot().range.end,
        "2027-01-31".parse::<chrono::NaiveDate>().unwrap()
    );
    assert_eq!(planner.snapshot().read_at, clock);
    let requests = fixture.requests.lock().unwrap();
    for request in requests.iter().filter(|r| r.starts_with("GET /4/")) {
        let target = request.split_whitespace().nth(1).unwrap();
        let url = fixture.base.join(target).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("from").unwrap(), "2027-01-01");
        assert_eq!(query.get("to").unwrap(), "2027-01-31");
    }
    drop(requests);
    all_gets(&fixture, 4);
}

#[tokio::test]
async fn an_absent_account_zone_blocks_current_month_before_any_tempo_request() {
    let fixture = Fixture::new(vec![identity("synthetic-user", None)]);
    let (tempo, jira) = clients(&fixture);
    assert!(matches!(
        Planner::connect_current_month_at(profile(), tempo, jira, now()).await,
        Err(PlannerError::Domain(Error::TimezoneUnavailable))
    ));
    assert!(fixture.requests.lock().unwrap()[0].starts_with("GET /rest/api/3/myself "));
    all_gets(&fixture, 1);
}

#[tokio::test]
async fn changed_timezone_during_today_refresh_cannot_publish_the_new_range_or_totals() {
    let mut responses = complete(600);
    responses.extend([
        user(),
        worklogs(vec![]),
        schedules(vec![]),
        identity("synthetic-user", Some("Pacific/Kiritimati")),
    ]);
    let fixture = Fixture::new(responses);
    let mut planner = connect(&fixture).await;
    let original = planner.snapshot().clone();
    assert_eq!(
        planner
            .refresh_current_month_at("2026-12-31T12:30:00Z".parse().unwrap())
            .await,
        Err(PlannerError::ChangedIdentity)
    );
    assert!(Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(planner.snapshot().summary.logged_seconds, 600);
    assert_eq!(
        planner.snapshot().range.start,
        "2026-10-16".parse::<chrono::NaiveDate>().unwrap()
    );
    all_gets(&fixture, 8);
}

#[tokio::test]
async fn a_failed_today_read_keeps_the_old_month_until_a_successful_explicit_retry() {
    let mut responses = complete(600);
    responses.extend([user(), reply(503, "private service failure")]);
    responses.extend([
        user(),
        worklogs(vec![worklog(7, "2026-11-01", 1200)]),
        schedules(vec![]),
        user(),
    ]);
    let fixture = Fixture::new(responses);
    let mut planner = connect(&fixture).await;
    let original = planner.snapshot().clone();
    let clock = "2026-11-01T12:00:00Z".parse().unwrap();
    assert_eq!(
        planner.refresh_current_month_at(clock).await,
        Err(PlannerError::Tempo(TransportError::Service))
    );
    assert!(Arc::ptr_eq(planner.snapshot(), &original));
    all_gets(&fixture, 6);
    planner.refresh_current_month_at(clock).await.unwrap();
    assert!(!Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(
        planner.snapshot().range.start,
        "2026-11-01".parse::<chrono::NaiveDate>().unwrap()
    );
    assert_eq!(
        planner.snapshot().range.end,
        "2026-11-30".parse::<chrono::NaiveDate>().unwrap()
    );
    assert_eq!(planner.snapshot().summary.logged_seconds, 1200);
    assert_eq!(original.summary.logged_seconds, 600);
    all_gets(&fixture, 10);
}

#[tokio::test]
async fn successful_range_keeps_service_dates_integer_totals_and_captured_context() {
    let fixture = Fixture::new(vec![
        user(),
        worklogs(vec![
            worklog(9007199254740993, "2026-10-16", 90061),
            worklog(8, "2026-10-16", 61),
            worklog(9, "2026-10-17", 600),
        ]),
        schedules(vec![("2026-10-16", 28800), ("2026-10-17", 0)]),
        user(),
    ]);
    let planner = connect(&fixture).await;
    let data = planner.snapshot();
    assert_eq!(data.account.as_str(), "synthetic-user");
    assert_eq!(data.time_zone.as_deref(), Some("America/New_York"));
    assert_eq!(data.read_at, now());
    assert_eq!(data.summary.logged_seconds, 90722);
    assert_eq!(data.summary.required_seconds, Some(28800));
    assert_eq!(data.summary.days[0].logged.seconds, 90122);
    assert_eq!(
        data.summary.days[0].logged.date,
        "2026-10-16".parse::<chrono::NaiveDate>().unwrap()
    );
    assert_eq!(data.summary.days[1].logged.seconds, 600);
    assert_eq!(
        data.summary.days[1]
            .schedule
            .as_ref()
            .unwrap()
            .required_seconds,
        0
    );
    assert!(
        data.summary.days[0]
            .logged
            .worklogs
            .iter()
            .any(|row| row.id.get() == 9007199254740993)
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests
        .iter()
        .filter(|request| request.starts_with("GET /4/"))
    {
        let target = request.split_whitespace().nth(1).unwrap();
        let url = fixture.base.join(target).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("from").unwrap(), "2026-10-16");
        assert_eq!(query.get("to").unwrap(), "2026-10-17");
        assert!(url.path().ends_with("/synthetic-user"));
    }
    drop(requests);
    all_gets(&fixture, 4);
}

#[tokio::test]
async fn failed_refresh_keeps_the_exact_snapshot_until_an_explicit_successful_refresh() {
    let mut responses = complete(600);
    responses.extend([user(), reply(503, "private failure body")]);
    responses.extend(complete_range(1200, "2026-10-18", "2026-10-19"));
    let fixture = Fixture::new(responses);
    let mut planner = connect(&fixture).await;
    let original = planner.snapshot().clone();
    assert_eq!(
        planner
            .refresh(
                range("2026-10-18", "2026-10-19"),
                now() + chrono::Duration::hours(1)
            )
            .await,
        Err(PlannerError::Tempo(TransportError::Service))
    );
    assert!(Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(planner.snapshot().summary.logged_seconds, 600);
    assert_eq!(planner.snapshot().read_at, now());
    assert_eq!(
        planner.snapshot().range.start,
        "2026-10-16".parse::<chrono::NaiveDate>().unwrap()
    );
    all_gets(&fixture, 6);
    planner
        .refresh(
            range("2026-10-18", "2026-10-19"),
            now() + chrono::Duration::hours(2),
        )
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(planner.snapshot().summary.logged_seconds, 1200);
    assert_eq!(original.summary.logged_seconds, 600);
    assert_eq!(
        planner.snapshot().range.start,
        "2026-10-18".parse::<chrono::NaiveDate>().unwrap()
    );
    all_gets(&fixture, 10);
    let requests = fixture.requests.lock().unwrap();
    for index in [5, 7, 8] {
        let target = requests[index].split_whitespace().nth(1).unwrap();
        let url = fixture.base.join(target).unwrap();
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "from" && value == "2026-10-18")
        );
    }
}

#[tokio::test]
async fn overflowing_service_totals_preserve_the_previous_snapshot() {
    let mut responses = complete(600);
    responses.extend([
        user(),
        worklogs(vec![
            worklog(7, "2026-10-16", i64::MAX as u64),
            worklog(8, "2026-10-16", i64::MAX as u64),
            worklog(9, "2026-10-16", i64::MAX as u64),
        ]),
        schedules(vec![]),
        user(),
    ]);
    let fixture = Fixture::new(responses);
    let mut planner = connect(&fixture).await;
    let original = planner.snapshot().clone();
    assert_eq!(
        planner
            .refresh(range("2026-10-16", "2026-10-17"), now())
            .await,
        Err(PlannerError::Domain(Error::TotalOverflow))
    );
    assert!(Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(planner.snapshot().summary.logged_seconds, 600);
    all_gets(&fixture, 8);
}

#[tokio::test]
async fn failed_schedule_does_not_publish_new_rows_and_missing_schedule_is_not_zero() {
    let mut responses = complete(600);
    responses.extend([
        user(),
        worklogs(vec![worklog(7, "2026-10-16", 1200)]),
        reply(503, "private schedule failure"),
    ]);
    let fixture = Fixture::new(responses);
    let mut planner = connect(&fixture).await;
    let original = planner.snapshot().clone();
    assert_eq!(
        planner
            .refresh(range("2026-10-16", "2026-10-17"), now())
            .await,
        Err(PlannerError::Tempo(TransportError::Service))
    );
    assert!(Arc::ptr_eq(planner.snapshot(), &original));
    assert_eq!(planner.snapshot().summary.logged_seconds, 600);
    all_gets(&fixture, 7);
    let missing = Fixture::new(vec![
        identity("synthetic-user", None),
        worklogs(vec![]),
        schedules(vec![]),
        identity("synthetic-user", None),
    ]);
    let planner = connect(&missing).await;
    assert!(planner.snapshot().time_zone.is_none());
    assert_eq!(planner.snapshot().summary.logged_seconds, 0);
    assert!(planner.snapshot().summary.required_seconds.is_none());
    assert!(
        planner
            .snapshot()
            .summary
            .days
            .iter()
            .all(|day| day.schedule.is_none())
    );
    all_gets(&missing, 4);
}

#[tokio::test]
async fn changed_account_or_timezone_before_or_after_loading_cannot_replace_the_range() {
    for changed in [
        identity("another-user", Some("America/New_York")),
        identity("synthetic-user", Some("Europe/London")),
    ] {
        for before in [true, false] {
            let mut responses = complete(600);
            if before {
                responses.push(changed.clone());
            } else {
                responses.extend([
                    user(),
                    worklogs(vec![worklog(7, "2026-10-16", 1200)]),
                    schedules(vec![]),
                    changed.clone(),
                ]);
            }
            let fixture = Fixture::new(responses);
            let mut planner = connect(&fixture).await;
            let original = planner.snapshot().clone();
            assert_eq!(
                planner
                    .refresh(
                        range("2026-10-16", "2026-10-17"),
                        now() + chrono::Duration::hours(1)
                    )
                    .await,
                Err(PlannerError::ChangedIdentity)
            );
            assert!(Arc::ptr_eq(planner.snapshot(), &original));
            assert_eq!(planner.snapshot().account.as_str(), "synthetic-user");
            assert_eq!(
                planner.snapshot().time_zone.as_deref(),
                Some("America/New_York")
            );
            all_gets(&fixture, if before { 5 } else { 8 });
        }
    }
}

#[tokio::test]
async fn invalid_profile_and_invalid_range_are_rejected_before_network_access() {
    let fixture = Fixture::new(vec![]);
    let (tempo, jira) = clients(&fixture);
    assert!(matches!(
        Planner::connect(
            Path::new("relative.json"),
            tempo.clone(),
            jira.clone(),
            range("2026-10-16", "2026-10-17"),
            now()
        )
        .await,
        Err(PlannerError::InvalidProfile)
    ));
    let invalid = DateRange {
        start: "2026-10-17".parse().unwrap(),
        end: "2026-10-16".parse().unwrap(),
    };
    assert!(matches!(
        Planner::connect(profile(), tempo, jira, invalid, now()).await,
        Err(PlannerError::Domain(Error::InvalidRange))
    ));
    all_gets(&fixture, 0);
}
