use super::*;

fn path() -> &'static Path {
    Path::new(if cfg!(windows) {
        "D:/synthetic-unused/todoist-profile"
    } else {
        "/synthetic-unused/todoist-profile"
    })
}
fn account() -> Id {
    Id::parse("synthetic-account").unwrap()
}
fn create(scope: WriteScope) -> WriteRequest {
    WriteRequest::create(
        scope,
        &CreateDraft {
            content: "Synthetic private task 界".into(),
            description: "One\nTwo".into(),
            project: Some(Id::parse("project").unwrap()),
            priority: Priority::from_api(4).unwrap(),
            due: Some(NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()),
        },
    )
    .unwrap()
}
fn mutate(bytes: &[u8], change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut saved: Value = serde_json::from_slice(bytes).unwrap();
    let mut command: Value = serde_json::from_str(saved["commands"].as_str().unwrap()).unwrap();
    change(&mut command);
    saved["commands"] = Value::String(serde_json::to_string(&command).unwrap());
    serde_json::to_vec(&saved).unwrap()
}

#[test]
fn every_supported_change_recovers_exact_original_bytes_and_ids_as_unconfirmed() {
    let scope = WriteScope::new();
    let target = Id::parse("task").unwrap();
    for request in [
        create(scope),
        WriteRequest::edit(
            scope,
            &target,
            &EditDraft {
                content: Some("Changed".into()),
                description: Some("".into()),
                priority: Some(Priority::from_api(2).unwrap()),
                due: Some(NaiveDate::from_ymd_opt(2026, 10, 18).unwrap()),
            },
        )
        .unwrap(),
        WriteRequest::complete(scope, &target).unwrap(),
        WriteRequest::reopen(scope, &target).unwrap(),
        WriteRequest::delete(scope, &target).unwrap(),
    ] {
        let attempt = WriteAttempt::new(request);
        let bytes = attempt.checkpoint(path(), &account()).unwrap();
        let new_scope = WriteScope::new();
        let mut recovered = WriteAttempt::restore(&bytes, path(), &account(), new_scope).unwrap();
        assert_eq!(recovered.phase(), WritePhase::Unconfirmed);
        assert!(!recovered.can_replace());
        assert!(matches!(
            recovered.begin(scope),
            Err(Error::WrongConnection)
        ));
        let retried = recovered.begin(new_scope).unwrap();
        assert_eq!(retried.commands(), attempt.request.commands());
        assert_eq!(retried.command_id(), attempt.request.command_id());
        assert_eq!(retried.temporary_id(), attempt.request.temporary_id());
        assert_eq!(retried.kind(), attempt.request.kind());
    }
}

#[test]
fn recovery_is_bound_to_the_original_profile_and_verified_account() {
    let attempt = WriteAttempt::new(create(WriteScope::new()));
    let bytes = attempt.checkpoint(path(), &account()).unwrap();
    assert!(matches!(
        WriteAttempt::restore(
            &bytes,
            Path::new(if cfg!(windows) {
                "D:/synthetic-unused/other-profile"
            } else {
                "/synthetic-unused/other-profile"
            }),
            &account(),
            WriteScope::new()
        ),
        Err(Error::WrongConnection)
    ));
    assert!(matches!(
        WriteAttempt::restore(
            &bytes,
            path(),
            &Id::parse("other-account").unwrap(),
            WriteScope::new()
        ),
        Err(Error::WrongConnection)
    ));
}

#[test]
fn malformed_or_expanded_commands_cannot_be_replayed() {
    let bytes = WriteAttempt::new(create(WriteScope::new()))
        .checkpoint(path(), &account())
        .unwrap();
    for invalid in [
        mutate(&bytes, |v| v[0]["type"] = json!("project_delete")),
        mutate(&bytes, |v| v[0]["uuid"] = json!(Uuid::nil().to_string())),
        mutate(&bytes, |v| v[0]["temp_id"] = v[0]["uuid"].clone()),
        mutate(&bytes, |v| v[0]["args"]["auto_parse_labels"] = json!(true)),
        mutate(&bytes, |v| v[0]["args"]["unsupported"] = json!(true)),
        mutate(&bytes, |v| v[0]["args"]["due"]["timezone"] = json!("UTC")),
        mutate(&bytes, |v| {
            let extra = v[0].clone();
            v.as_array_mut().unwrap().push(extra);
        }),
        mutate(&bytes, |v| v[0]["args"]["content"] = json!("\u{0}bad")),
        mutate(&bytes, |v| v[0]["args"]["priority"] = json!(8)),
        b"not-json".to_vec(),
        vec![b' '; MAX_CHECKPOINT_BYTES + 1],
    ] {
        assert!(WriteAttempt::restore(&invalid, path(), &account(), WriteScope::new()).is_err());
    }
}

#[test]
fn terminal_results_cannot_be_checkpointed_for_replay_but_inflight_can() {
    for outcome in [
        WriteOutcome::Confirmed(Id::parse("created").unwrap()),
        WriteOutcome::Rejected,
    ] {
        let scope = WriteScope::new();
        let mut attempt = WriteAttempt::new(create(scope));
        let request = attempt.begin(scope).unwrap();
        let bytes = attempt.checkpoint(path(), &account()).unwrap();
        assert_eq!(
            WriteAttempt::restore(&bytes, path(), &account(), WriteScope::new())
                .unwrap()
                .phase(),
            WritePhase::Unconfirmed
        );
        assert!(attempt.observe(request.command_id(), outcome));
        assert!(matches!(
            attempt.checkpoint(path(), &account()),
            Err(Error::WriteBlocked)
        ));
    }
}

#[test]
fn unsupported_envelopes_and_duplicate_command_fields_are_rejected() {
    let bytes = WriteAttempt::new(create(WriteScope::new()))
        .checkpoint(path(), &account())
        .unwrap();
    let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
    envelope["version"] = json!(2);
    assert!(
        WriteAttempt::restore(
            &serde_json::to_vec(&envelope).unwrap(),
            path(),
            &account(),
            WriteScope::new()
        )
        .is_err()
    );
    envelope["version"] = json!(1);
    envelope["token"] = json!("synthetic-must-never-be-persisted");
    assert!(
        WriteAttempt::restore(
            &serde_json::to_vec(&envelope).unwrap(),
            path(),
            &account(),
            WriteScope::new()
        )
        .is_err()
    );
    envelope.as_object_mut().unwrap().remove("token");
    let commands = envelope["commands"].as_str().unwrap().replacen(
        "\"type\":",
        "\"type\":\"item_delete\",\"type\":",
        1,
    );
    envelope["commands"] = json!(commands);
    assert!(
        WriteAttempt::restore(
            &serde_json::to_vec(&envelope).unwrap(),
            path(),
            &account(),
            WriteScope::new()
        )
        .is_err()
    );
}
