use super::*;
use chrono::NaiveDate;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

fn draft() -> CreateDraft {
    CreateDraft {
        content: "  Synthetic @literal task 界  ".into(),
        description: "Line one\nLine two".into(),
        project: Some(Id::parse("synthetic-project").unwrap()),
        priority: Priority::from_api(4).unwrap(),
        due: Some(NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()),
    }
}
fn commands(request: &WriteRequest) -> Value {
    serde_json::from_str(request.commands()).unwrap()
}
fn success(request: &WriteRequest) -> Vec<u8> {
    let mut value = json!({"sync_status":{request.command_id().to_string():"ok"}});
    if let Some(temporary) = request.temporary_id() {
        value["temp_id_mapping"] = json!({temporary.to_string():"created-task"});
    }
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn creating_captures_one_command_and_valid_uuids_without_implicit_label_writes() {
    let request = WriteRequest::create(WriteScope::new(), &draft()).unwrap();
    assert_eq!(request.command_id().get_version_num(), 4);
    assert_eq!(request.temporary_id().unwrap().get_version_num(), 4);
    assert_ne!(request.temporary_id().unwrap(), request.command_id());
    let body = commands(&request);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["type"], "item_add");
    assert_eq!(body[0]["args"]["content"], "Synthetic @literal task 界");
    assert_eq!(body[0]["args"]["description"], "Line one\nLine two");
    assert_eq!(body[0]["args"]["due"], json!({"date":"2026-10-16"}));
    assert_eq!(body[0]["args"]["priority"], 4);
    assert_eq!(body[0]["args"]["project_id"], "synthetic-project");
    assert_eq!(body[0]["args"]["auto_parse_labels"], false);
}

#[test]
fn draft_edits_and_unknown_retries_cannot_rebuild_or_replace_the_captured_command() {
    let scope = WriteScope::new();
    let mut editable = draft();
    let request = WriteRequest::create(scope, &editable).unwrap();
    let original = request.commands().to_owned();
    editable.content = "Different unsaved draft".into();
    editable.project = Some(Id::parse("different-project").unwrap());
    let mut attempt = WriteAttempt::new(request);
    let first = attempt.begin(scope).unwrap();
    assert_eq!(attempt.phase(), WritePhase::InFlight);
    assert!(!attempt.can_replace());
    assert!(matches!(attempt.begin(scope), Err(Error::WriteBlocked)));
    assert!(attempt.observe(first.command_id(), WriteOutcome::Unconfirmed));
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
    assert!(!attempt.can_replace());
    let retried = attempt.begin(scope).unwrap();
    assert!(Arc::ptr_eq(&first, &retried));
    assert_eq!(retried.commands(), original);
    assert!(attempt.observe(retried.command_id(), retried.decode(&success(&retried))));
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    assert_eq!(attempt.task_id().unwrap().as_str(), "created-task");
    assert!(attempt.can_replace());
}

#[test]
fn another_connection_cannot_begin_or_retry_a_change_and_stale_replies_do_not_finish_it() {
    let scope = WriteScope::new();
    let mut attempt = WriteAttempt::new(WriteRequest::create(scope, &draft()).unwrap());
    assert!(matches!(
        attempt.begin(WriteScope::new()),
        Err(Error::WrongConnection)
    ));
    assert_eq!(attempt.phase(), WritePhase::Ready);
    let sent = attempt.begin(scope).unwrap();
    assert!(!attempt.observe(Uuid::new_v4(), WriteOutcome::Rejected));
    assert_eq!(attempt.phase(), WritePhase::InFlight);
    assert!(attempt.observe(sent.command_id(), WriteOutcome::Unconfirmed));
    assert!(matches!(
        attempt.begin(WriteScope::new()),
        Err(Error::WrongConnection)
    ));
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
}

#[test]
fn missing_or_foreign_status_and_missing_creation_mapping_remain_unconfirmed() {
    let request = WriteRequest::create(WriteScope::new(), &draft()).unwrap();
    for value in [
        json!(null),
        json!({"sync_status":{"another-command":"ok"}}),
        json!({"sync_status":{request.command_id().to_string():true}}),
        json!({"sync_status":{request.command_id().to_string():"ok"}}),
        json!({"sync_status":{request.command_id().to_string():"ok"},
            "temp_id_mapping":{request.temporary_id().unwrap().to_string():"../invalid"}}),
        json!({"sync_status":{request.command_id().to_string():{"error_code":0,"error":"unknown"}}}),
    ] {
        assert!(matches!(
            request.decode(&serde_json::to_vec(&value).unwrap()),
            WriteOutcome::Unconfirmed
        ));
    }
    assert!(matches!(
        request.decode(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
        WriteOutcome::Unconfirmed
    ));
}

#[test]
fn explicit_rejection_has_no_private_service_text_and_cannot_replay_as_a_new_send() {
    let scope = WriteScope::new();
    let request = WriteRequest::create(scope, &draft()).unwrap();
    let rejected = json!({"sync_status":{request.command_id().to_string():
        {"error_code":22,"error":"never-log-private-provider-payload"}}});
    let outcome = request.decode(&serde_json::to_vec(&rejected).unwrap());
    assert!(matches!(outcome, WriteOutcome::Rejected));
    let mut attempt = WriteAttempt::new(request);
    let sent = attempt.begin(scope).unwrap();
    assert!(attempt.observe(sent.command_id(), outcome));
    assert_eq!(attempt.phase(), WritePhase::Rejected);
    assert!(attempt.can_replace());
    assert!(matches!(attempt.begin(scope), Err(Error::WriteBlocked)));
}

#[test]
fn confirmation_survives_late_failures_and_cannot_be_resent_after_refresh_failure() {
    let scope = WriteScope::new();
    let mut attempt = WriteAttempt::new(WriteRequest::create(scope, &draft()).unwrap());
    let sent = attempt.begin(scope).unwrap();
    assert!(attempt.observe(sent.command_id(), sent.decode(&success(&sent))));
    assert!(!attempt.observe(sent.command_id(), WriteOutcome::Unconfirmed));
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    assert_eq!(attempt.task_id().unwrap().as_str(), "created-task");
    assert!(matches!(attempt.begin(scope), Err(Error::WriteBlocked)));
}

#[test]
fn editing_only_content_preserves_existing_due_metadata_and_all_other_fields() {
    let target = Id::parse("existing-timed-task").unwrap();
    let request = WriteRequest::edit(
        WriteScope::new(),
        &target,
        &EditDraft {
            content: Some("Updated title".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(commands(&request)[0]["type"], "item_update");
    assert_eq!(
        commands(&request)[0]["args"],
        json!({"id":"existing-timed-task","content":"Updated title"})
    );
    assert!(request.temporary_id().is_none());
    assert!(
        matches!(request.decode(&success(&request)),WriteOutcome::Confirmed(id) if id.as_str()=="existing-timed-task")
    );
    assert!(matches!(
        WriteRequest::edit(WriteScope::new(), &target, &EditDraft::default()),
        Err(Error::InvalidDraft)
    ));
}

#[test]
fn completion_delegates_recurrence_to_item_close_and_deletion_sends_only_its_frozen_id() {
    let target = Id::parse("recurring-task").unwrap();
    for (request, kind) in [
        (
            WriteRequest::complete(WriteScope::new(), &target).unwrap(),
            "item_close",
        ),
        (
            WriteRequest::reopen(WriteScope::new(), &target).unwrap(),
            "item_uncomplete",
        ),
        (
            WriteRequest::delete(WriteScope::new(), &target).unwrap(),
            "item_delete",
        ),
    ] {
        let body = commands(&request);
        assert_eq!(body[0]["type"], kind);
        assert_eq!(body[0]["args"], json!({"id":"recurring-task"}));
        assert!(request.temporary_id().is_none());
        assert!(
            matches!(request.decode(&success(&request)),WriteOutcome::Confirmed(id) if id.as_str()=="recurring-task")
        );
    }
}

#[test]
fn invalid_fields_and_json_expansion_are_rejected_before_a_request_can_exist() {
    for text in ["", " \n ", "task\u{0}name"] {
        let mut invalid = draft();
        invalid.content = text.into();
        assert!(matches!(
            WriteRequest::create(WriteScope::new(), &invalid),
            Err(Error::InvalidDraft)
        ));
    }
    let mut large = draft();
    large.description = "\\".repeat(128 * 1024);
    assert!(matches!(
        WriteRequest::create(WriteScope::new(), &large),
        Err(Error::InvalidDraft)
    ));
    let mut invalid = draft();
    invalid.due = Some(NaiveDate::from_ymd_opt(0, 1, 1).unwrap());
    assert!(matches!(
        WriteRequest::create(WriteScope::new(), &invalid),
        Err(Error::InvalidDraft)
    ));
}
