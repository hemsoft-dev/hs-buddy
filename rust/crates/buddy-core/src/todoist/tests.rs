use super::*;
use chrono::{DateTime, FixedOffset, NaiveDate};
use serde_json::{Value, json};

fn task(due: Value) -> Value {
    json!({
        "id":"synthetic-task", "content":"Ship a synthetic change 界",
        "description":"Private synthetic notes", "project_id":"synthetic-project",
        "parent_id":null, "priority":4, "due":due, "labels":["planning"],
        "checked":false, "added_at":"2026-10-16T10:00:00Z", "child_order":2,
    })
}
fn page(rows: Vec<Value>, cursor: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"results":rows,"next_cursor":cursor})).unwrap()
}
fn due(value: Value) -> DueDate {
    parse_tasks(&page(vec![task(value)], Value::Null))
        .unwrap()
        .items
        .remove(0)
        .due
        .unwrap()
}
fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

#[test]
fn project_workspace_wire_types_and_role_casing_preserve_team_permissions() {
    for workspace in [
        json!(12345),
        json!(0),
        json!("12345"),
        json!("temporary-workspace"),
    ] {
        let row = json!({"id":"project","name":"Synthetic team project","color":"blue","child_order":0,
            "workspace_id":workspace,"role":"complete_only"});
        let project = parse_projects(&page(vec![row.clone()], Value::Null))
            .unwrap()
            .items
            .remove(0);
        assert!(project.allows(WriteKind::Complete));
        assert!(
            !project.allows(WriteKind::Create)
                && !project.allows(WriteKind::Edit)
                && !project.allows(WriteKind::Delete)
        );
        let mut missing = row;
        missing.as_object_mut().unwrap().remove("role");
        let project = parse_projects(&page(vec![missing], Value::Null))
            .unwrap()
            .items
            .remove(0);
        assert!(!project.allows(WriteKind::Complete));
    }
    for malformed in [
        json!(true),
        json!(-1),
        json!(1.5),
        json!([]),
        json!({}),
        json!(""),
    ] {
        let row = json!({"id":"project","name":"Synthetic team project","color":"blue","child_order":0,
            "workspace_id":malformed,"role":"READ_WRITE"});
        assert!(matches!(
            parse_projects(&page(vec![row], Value::Null)),
            Err(Error::InvalidResponse)
        ));
    }
}

#[test]
fn current_api_fields_keep_completion_order_content_and_priority() {
    let parsed = parse_tasks(&page(vec![task(Value::Null)], json!("opaque-cursor"))).unwrap();
    assert_eq!(parsed.next_cursor.as_deref(), Some("opaque-cursor"));
    let task = &parsed.items[0];
    assert_eq!(task.id.as_str(), "synthetic-task");
    assert_eq!(task.content, "Ship a synthetic change 界");
    assert_eq!(task.description, "Private synthetic notes");
    assert_eq!(task.project_id.as_str(), "synthetic-project");
    assert_eq!(task.priority.api_value(), 4);
    assert_eq!(task.priority.label(), "P1");
    assert!(!task.completed && !task.deleted);
    assert_eq!(task.order, 2);
    assert_eq!(task.created_at.unwrap().timestamp(), 1792144800);
    assert_eq!(task.labels, ["planning"]);
}

#[test]
fn legacy_field_names_are_supported_without_defaulting_missing_completion() {
    let mut row = task(Value::Null);
    let object = row.as_object_mut().unwrap();
    object.remove("checked");
    object.remove("child_order");
    object.remove("added_at");
    object.insert("is_completed".into(), json!(true));
    object.insert("order".into(), json!(7));
    object.insert("created_at".into(), json!(null));
    let parsed = parse_tasks(&page(vec![row.clone()], Value::Null)).unwrap();
    assert!(parsed.items[0].completed);
    assert_eq!(parsed.items[0].order, 7);
    assert!(parsed.items[0].created_at.is_none());
    row.as_object_mut().unwrap().remove("is_completed");
    assert!(parse_tasks(&page(vec![row], Value::Null)).is_err());
}

#[test]
fn priority_mapping_and_malformed_rows_do_not_silently_invent_values() {
    for (value, label) in [(1, "P4"), (2, "P3"), (3, "P2"), (4, "P1")] {
        assert_eq!(Priority::from_api(value).unwrap().label(), label);
    }
    for value in [json!(0), json!(5), json!(1.5), json!("4")] {
        let mut row = task(Value::Null);
        row["priority"] = value;
        assert!(parse_tasks(&page(vec![row], Value::Null)).is_err());
    }
    for identifier in ["", "../private", "bad/id", "task\nid"] {
        let mut row = task(Value::Null);
        row["id"] = json!(identifier);
        assert!(parse_tasks(&page(vec![row], Value::Null)).is_err());
    }
}

#[test]
fn missing_due_data_cannot_silently_reclassify_a_task_as_unscheduled() {
    let mut row = task(Value::Null);
    row.as_object_mut().unwrap().remove("due");
    assert!(parse_tasks(&page(vec![row], Value::Null)).is_err());
    let parsed = parse_tasks(&page(vec![task(Value::Null)], Value::Null)).unwrap();
    assert!(parsed.items[0].due.is_none());
}

#[test]
fn calendar_and_floating_dates_keep_their_wall_date_on_both_sides_of_utc() {
    for value in ["2026-10-16", "2026-10-16T12:00:00.500"] {
        let due = due(json!({"date":value}));
        for zone in [
            FixedOffset::west_opt(12 * 3600).unwrap(),
            FixedOffset::east_opt(14 * 3600).unwrap(),
        ] {
            assert_eq!(due.day_in(&zone), date(16));
        }
    }
    let calendar = due(json!({"date":"2026-10-16"}));
    let floating = due(json!({"date":"2026-10-16T12:00:00"}));
    let now = DateTime::parse_from_rfc3339("2026-10-16T13:00:00-04:00").unwrap();
    assert!(!calendar.overdue_at(&now));
    assert!(floating.overdue_at(&now));
}

#[test]
fn explicit_instants_convert_across_midnight_and_preserve_legacy_recurrence_data() {
    let due = due(
        json!({"date":"2026-10-16","datetime":"2026-10-17T00:30:00Z",
        "timezone":"America/New_York","string":"every Friday at 8:30pm","is_recurring":true}),
    );
    assert_eq!(
        due.day_in(&FixedOffset::west_opt(4 * 3600).unwrap()),
        date(16)
    );
    assert_eq!(
        due.day_in(&FixedOffset::east_opt(14 * 3600).unwrap()),
        date(17)
    );
    assert_eq!(due.source_date, "2026-10-16");
    assert_eq!(due.source_datetime.as_deref(), Some("2026-10-17T00:30:00Z"));
    assert_eq!(due.timezone.as_deref(), Some("America/New_York"));
    assert_eq!(due.description.as_deref(), Some("every Friday at 8:30pm"));
    assert!(due.recurring);
    let now = DateTime::parse_from_rfc3339("2026-10-16T21:00:00-04:00").unwrap();
    assert!(due.overdue_at(&now));
}

#[test]
fn invalid_or_conflicting_dates_are_rejected_without_falling_back_to_date_only() {
    for value in ["2026-02-30", "0000-01-01", "2026-10-16Tbad", "not-a-date"] {
        assert!(parse_tasks(&page(vec![task(json!({"date":value}))], Value::Null)).is_err());
    }
    for value in [
        json!({"date":"2026-10-16","datetime":"not-an-instant"}),
        json!({"date":"2026-10-16T12:00:00Z","datetime":"2026-10-16T13:00:00Z"}),
        json!({"date":"2026-10-16","timezone":"private\nzone"}),
    ] {
        assert!(parse_tasks(&page(vec![task(value)], Value::Null)).is_err());
    }
}

#[test]
fn pagination_requires_a_real_end_marker_and_preserves_empty_intermediate_pages() {
    assert!(parse_tasks(br#"{"results":[]}"#).is_err());
    assert!(parse_tasks(b"null").is_err());
    assert!(parse_tasks(b"[]").is_err());
    assert!(
        parse_tasks(&page(vec![], Value::Null))
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        parse_tasks(&page(vec![], json!("next-page")))
            .unwrap()
            .next_cursor
            .as_deref(),
        Some("next-page")
    );
    for cursor in [
        json!(""),
        json!("\n"),
        json!({"cursor":"wrong-shape"}),
        json!("x".repeat(MAX_CURSOR_BYTES + 1)),
    ] {
        assert!(parse_tasks(&page(vec![], cursor)).is_err());
    }
}

#[test]
fn duplicate_ids_oversize_pages_and_private_error_payloads_are_never_partial_success() {
    assert!(
        parse_tasks(&page(
            vec![task(Value::Null), task(Value::Null)],
            Value::Null
        ))
        .is_err()
    );
    let rows = (0..=MAX_PAGE_ITEMS)
        .map(|index| {
            let mut row = task(Value::Null);
            row["id"] = json!(format!("task-{index}"));
            row
        })
        .collect();
    assert!(parse_tasks(&page(rows, Value::Null)).is_err());
    assert!(matches!(
        parse_tasks(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Err(Error::ResponseTooLarge)
    ));
    let mut private = task(Value::Null);
    private["id"] = json!("never-log-private/identifier");
    let error = match parse_tasks(&page(vec![private], Value::Null)) {
        Err(error) => error,
        Ok(_) => panic!("Invalid identifier was accepted"),
    };
    assert_eq!(
        error.to_string(),
        "The Todoist task or project identifier is unsupported."
    );
}

#[test]
fn projects_keep_parent_context_and_explicit_archived_flags() {
    let project = json!({"id":"project-child","name":"Synthetic project","color":"berry_red",
        "parent_id":"project-parent","child_order":3,"is_archived":true,"is_deleted":false});
    let parsed = parse_projects(&page(vec![project.clone()], Value::Null)).unwrap();
    assert_eq!(
        parsed.items[0].parent_id.as_ref().unwrap().as_str(),
        "project-parent"
    );
    assert_eq!(parsed.items[0].name, "Synthetic project");
    assert_eq!(parsed.items[0].color, "berry_red");
    assert_eq!(parsed.items[0].order, 3);
    assert!(parsed.items[0].archived && !parsed.items[0].deleted);
    assert!(parse_projects(&page(vec![project.clone(), project], Value::Null)).is_err());
    assert!(
        parse_projects(&page(vec![], Value::Null))
            .unwrap()
            .items
            .is_empty()
    );
}
