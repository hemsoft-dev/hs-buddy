use chrono::{FixedOffset, TimeZone};
use serde_json::{Value, json};

use super::*;
use crate::todoist::parse_tasks;

fn task(id: &str, due: Value, priority: u8, order: i64) -> Value {
    json!({"id":id,"content":"Synthetic task","description":"","project_id":"project","due":due,"priority":priority,"child_order":order,"checked":false,"labels":[]})
}
fn tasks(rows: Vec<Value>) -> Vec<Task> {
    parse_tasks(&serde_json::to_vec(&json!({"results":rows,"next_cursor":null})).unwrap())
        .unwrap()
        .items
}

#[test]
fn explicit_instants_cross_midnight_while_calendar_and_floating_values_keep_their_day() {
    let rows = tasks(vec![
        task("instant", json!({"date":"2026-10-17T01:00:00Z"}), 1, 0),
        task("calendar", json!({"date":"2026-10-17"}), 1, 0),
        task("floating", json!({"date":"2026-10-17T01:00:00"}), 1, 0),
        task("overdue", json!({"date":"2026-10-15"}), 1, 0),
    ]);
    let zone = FixedOffset::west_opt(4 * 3600).unwrap();
    let now = zone.with_ymd_and_hms(2026, 10, 16, 22, 0, 0).unwrap();
    let groups = schedule(&rows, &now, 2).unwrap();
    assert!(groups[0].kind == DayKind::Overdue);
    assert_eq!(groups[0].tasks[0].id.as_str(), "overdue");
    assert!(groups[1].kind == DayKind::Date(now.date_naive()));
    assert_eq!(groups[1].tasks[0].id.as_str(), "instant");
    assert_eq!(
        groups[2]
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["calendar", "floating"]
    );
}

#[test]
fn priority_order_and_identity_sort_is_stable_and_completed_or_deleted_tasks_stay_hidden() {
    let mut done = task("done", json!({"date":"2026-10-16"}), 4, -1);
    done["checked"] = json!(true);
    let mut deleted = task("deleted", json!({"date":"2026-10-16"}), 4, -2);
    deleted["is_deleted"] = json!(true);
    let rows = tasks(vec![
        task("low", json!({"date":"2026-10-16"}), 1, 0),
        task("high-b", json!({"date":"2026-10-16"}), 4, 2),
        task("high-a", json!({"date":"2026-10-16"}), 4, 2),
        task("first", json!({"date":"2026-10-16"}), 4, 1),
        done,
        deleted,
    ]);
    let now = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 10, 16, 12, 0, 0)
        .unwrap();
    let groups = schedule(&rows, &now, 1).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0]
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "high-a", "high-b", "low"]
    );
}

#[test]
fn project_includes_undated_tasks_and_future_dates_without_stealing_another_projects_tasks() {
    let mut other = task("other", Value::Null, 4, 0);
    other["project_id"] = json!("other-project");
    let rows = tasks(vec![
        task("undated", Value::Null, 1, 0),
        task("future", json!({"date":"2030-01-01"}), 1, 0),
        other,
    ]);
    let result = project_tasks(&rows, &Id::parse("project").unwrap());
    assert_eq!(
        result
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["future", "undated"]
    );
    let now = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 10, 16, 0, 0, 0)
        .unwrap();
    assert!(
        schedule(&rows, &now, 7)
            .unwrap()
            .iter()
            .all(|group| group.tasks.is_empty())
    );
}

#[test]
fn empty_calendar_sections_cover_leap_day_and_end_is_exclusive() {
    let now = FixedOffset::east_opt(14 * 3600)
        .unwrap()
        .with_ymd_and_hms(2028, 2, 28, 23, 59, 0)
        .unwrap();
    let rows = tasks(vec![task("end", json!({"date":"2028-03-02"}), 1, 0)]);
    let groups = schedule(&rows, &now, 3).unwrap();
    let dates: Vec<_> = groups
        .iter()
        .map(|group| match group.kind {
            DayKind::Date(day) => day.to_string(),
            _ => panic!("unexpected overdue group"),
        })
        .collect();
    assert_eq!(dates, ["2028-02-28", "2028-02-29", "2028-03-01"]);
    assert!(groups.iter().all(|group| group.tasks.is_empty()));
    assert!(matches!(schedule(&rows, &now, 0), Err(Error::InvalidDraft)));
    assert!(matches!(
        schedule(&rows, &now, 32),
        Err(Error::InvalidDraft)
    ));
}
