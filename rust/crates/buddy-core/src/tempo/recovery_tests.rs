use super::*;
use crate::tempo::{AccountId, IssueId, WorklogId, WriteOutcome};

fn context() -> Context {
    Context {
        profile: "synthetic-profile-digest".into(),
        origin: "https://synthetic.atlassian.net/".into(),
        account: "synthetic-user".into(),
    }
}
fn create() -> Draft {
    Draft::create(
        AccountId::parse("synthetic-user").unwrap(),
        IssueId::parse(9007199254740993).unwrap(),
        "2026-10-16".parse().unwrap(),
        9007199254740993,
    )
    .unwrap()
}
fn row() -> Worklog {
    parse_worklog(&serde_json::to_vec(&json!({"tempoWorklogId":123,"self":"https://api.tempo.io/4/worklogs/123", "issue":{"id":456,"self":"https://api.tempo.io/4/issues/456"},
        "author":{"accountId":"synthetic-user"}, "timeSpentSeconds":600,"billableSeconds":-1,
        "startDate":"2026-10-16","startTime":"01:02:03","startDateTimeUtc":"2026-10-16T05:02:03Z",
        "description":"Original private description 界\nSecond line", "createdAt":"2026-10-01T01:00:00Z","updatedAt":"2026-10-16T02:00:00.123Z",
        "attributes":{"self":"https://api.tempo.io/4/worklogs/123/work-attribute-values","values":[{"key":"_Account_","value":"billable"},{"key":"other","value":"keep\nthis"},{"key":"absent"}]}})).unwrap()).unwrap()
}
fn mutate(bytes: &[u8], change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    change(&mut value);
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn appended_terminal_marker_is_exactly_correlated_and_never_accepts_partial_or_extra_records() {
    let client = Client::new("synthetic-token").unwrap();
    let mut attempt = client.capture(create()).unwrap();
    let pending = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    assert!(attempt.observe(
        request.operation_id(),
        WriteOutcome::Confirmed(WorklogId::parse(123).unwrap())
    ));
    let marker = acknowledgement(&attempt, client.write_scope(), &pending).unwrap();
    let mut full = pending.clone();
    full.extend_from_slice(&marker);
    let restored = decode(&full, &context()).unwrap();
    assert!(restored.phase() == WritePhase::Confirmed);
    assert_eq!(restored.confirmed_id().unwrap().get(), 123);
    for end in 1..marker.len() {
        let mut truncated = pending.clone();
        truncated.extend_from_slice(&marker[..end]);
        assert!(matches!(
            decode(&truncated, &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    for (field, value) in [
        ("version", json!(2)),
        ("operation", json!(Uuid::new_v4().to_string())),
        ("checkpoint_sha256", json!("0".repeat(64))),
        ("outcome", json!({"status":"confirmed","worklog_id":0})),
        ("unsupported", json!(true)),
    ] {
        let mut changed = pending.clone();
        changed.extend_from_slice(&mutate(&marker, |stored| stored[field] = value));
        assert!(
            matches!(decode(&changed, &context()), Err(Error::InvalidRecovery)),
            "{field}"
        );
    }
    for extra in [marker.clone(), b"{}".to_vec(), b"garbage".to_vec()] {
        let mut changed = full.clone();
        changed.extend_from_slice(&extra);
        assert!(matches!(
            decode(&changed, &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    let mut inline = mutate(&pending, |stored| {
        stored["outcome"] = json!({"status":"rejected"})
    });
    inline.extend_from_slice(&marker);
    assert!(matches!(
        decode(&inline, &context()),
        Err(Error::InvalidRecovery)
    ));
    let mut rejected = client.capture(create()).unwrap();
    let rejected_pending = encode(
        rejected.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    let request = rejected.begin(client.write_scope()).unwrap();
    assert!(rejected.observe(request.operation_id(), WriteOutcome::Rejected));
    let mut rejected_bytes = rejected_pending.clone();
    rejected_bytes.extend_from_slice(
        &acknowledgement(&rejected, client.write_scope(), &rejected_pending).unwrap(),
    );
    assert!(decode(&rejected_bytes, &context()).unwrap().phase() == WritePhase::Rejected);
}

#[test]
fn acknowledgement_cannot_confirm_a_different_body_with_the_same_operation_id() {
    let client = Client::new("synthetic-token").unwrap();
    let mut attempt = client.capture(create()).unwrap();
    let pending = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    assert!(attempt.observe(
        request.operation_id(),
        WriteOutcome::Confirmed(WorklogId::parse(123).unwrap())
    ));
    let marker = acknowledgement(&attempt, client.write_scope(), &pending).unwrap();
    let mut different_draft = create();
    different_draft
        .set_description(Some(
            "Changed private body with the same operation".to_owned(),
        ))
        .unwrap();
    let different_body = different_draft.encode().unwrap();
    let mut changed = mutate(&pending, |stored| {
        stored["body"] = json!(String::from_utf8(different_body.clone()).unwrap());
    });
    let valid_pending = decode(&changed, &context()).unwrap();
    assert!(valid_pending.phase() == WritePhase::Unconfirmed);
    assert_eq!(valid_pending.original_body(), different_body);
    changed.extend_from_slice(&marker);
    assert!(matches!(
        decode(&changed, &context()),
        Err(Error::InvalidRecovery)
    ));
}

#[test]
fn acknowledged_recovery_remains_terminal_with_original_payload_and_operation() {
    let client = Client::new("synthetic-token").unwrap();
    let original = row();
    for draft in [
        create(),
        Draft::edit(&original).unwrap(),
        Draft::delete(&original).unwrap(),
    ] {
        let attempt = client.capture(draft).unwrap();
        let request = attempt.pending_request(client.write_scope()).unwrap();
        let pending = encode(request, &context()).unwrap();
        assert_eq!(
            decode(&pending, &context()).unwrap().phase(),
            WritePhase::Unconfirmed
        );
        let id = request
            .draft()
            .worklog_id()
            .map_or(9007199254740993, |id| id.get());
        for (outcome, phase) in [
            (
                json!({"status":"confirmed","worklog_id":id}),
                WritePhase::Confirmed,
            ),
            (json!({"status":"rejected"}), WritePhase::Rejected),
        ] {
            let bytes = mutate(&pending, |stored| stored["outcome"] = outcome);
            let restored = decode(&bytes, &context())
                .expect("An acknowledged worklog must remain terminal after restart");
            assert_eq!(restored.phase(), phase);
            assert_eq!(restored.original_body(), request.body());
            assert_eq!(restored.operation_id(), request.operation_id());
            assert!(restored.draft().baseline() == request.draft().baseline());
        }
    }
}

#[test]
fn deletion_recovery_preserves_an_empty_body_and_full_target_for_inspection_only() {
    let client = Client::new("synthetic-token").unwrap();
    let original = row();
    let attempt = client.capture(Draft::delete(&original).unwrap()).unwrap();
    let request = attempt.pending_request(client.write_scope()).unwrap();
    let bytes = encode(request, &context()).unwrap();
    let restored = decode(&bytes, &context()).unwrap();
    assert_eq!(restored.phase(), WritePhase::Unconfirmed);
    assert_eq!(restored.operation_id(), request.operation_id());
    assert_eq!(restored.draft().kind(), DraftKind::Delete);
    assert!(restored.original_body().is_empty());
    assert!(restored.draft().baseline() == Some(&original));
    for (name, value) in [
        ("body", json!("{}")),
        ("baseline", Value::Null),
        ("target", json!(124)),
        ("kind", json!("update")),
    ] {
        assert!(matches!(
            decode(&mutate(&bytes, |stored| stored[name] = value), &context()),
            Err(Error::InvalidRecovery)
        ));
    }
}

#[test]
fn recovery_retains_exact_payload_uuid_and_full_baseline_but_never_becomes_ready() {
    let client = Client::new("synthetic-token").unwrap();
    let original = row();
    let mut edit = Draft::edit(&original).unwrap();
    edit.set_description(Some("Edited local text".into()))
        .unwrap();
    edit.set_duration(1200).unwrap();
    for draft in [create(), edit] {
        let attempt = client.capture(draft).unwrap();
        let request = attempt.pending_request(client.write_scope()).unwrap();
        let bytes = encode(request, &context()).unwrap();
        let restored = decode(&bytes, &context()).unwrap();
        assert_eq!(restored.phase(), WritePhase::Unconfirmed);
        assert_eq!(restored.operation_id(), request.operation_id());
        assert_eq!(restored.original_body(), request.body());
        if restored.draft().kind() == DraftKind::Update {
            assert!(restored.draft().check_update_baseline(&original).is_ok());
            let mut changed = original.clone();
            changed.description = Some("same timestamp, different private description".into());
            assert_eq!(
                restored.draft().check_update_baseline(&changed),
                Err(Error::ChangedWhileLoading)
            );
        } else {
            assert!(restored.draft().issue_id().unwrap().get() == 9007199254740993);
        }
    }
}

#[test]
fn all_pending_phases_recover_for_reconciliation_and_terminal_phases_cannot_checkpoint() {
    let client = Client::new("synthetic-token").unwrap();
    let mut attempt = client.capture(create()).unwrap();
    let before = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    let during = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    assert_eq!(before, during);
    assert!(attempt.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    let after = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        decode(&after, &context()).unwrap().phase(),
        WritePhase::Unconfirmed
    );
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    for outcome in [
        WriteOutcome::Confirmed(WorklogId::parse(1).unwrap()),
        WriteOutcome::Rejected,
    ] {
        let mut attempt = client.capture(create()).unwrap();
        let request = attempt.begin(client.write_scope()).unwrap();
        attempt.observe(request.operation_id(), outcome);
        assert!(matches!(
            attempt.pending_request(client.write_scope()),
            Err(Error::WriteBlocked)
        ));
    }
}

#[test]
fn profile_origin_author_and_live_connection_scope_cannot_be_rebound() {
    let client = Client::new("synthetic-token").unwrap();
    let attempt = client.capture(create()).unwrap();
    let request = attempt.pending_request(client.write_scope()).unwrap();
    let bytes = encode(request, &context()).unwrap();
    for name in ["profile", "origin", "account"] {
        let mut other = context();
        match name {
            "profile" => other.profile = "another-profile".into(),
            "origin" => other.origin = "https://another.atlassian.net/".into(),
            _ => other.account = "another-user".into(),
        }
        assert!(matches!(
            decode(&bytes, &other),
            Err(Error::WrongConnection)
        ));
        if name == "account" {
            assert!(matches!(
                encode(request, &other),
                Err(Error::WrongConnection)
            ));
        }
    }
    let other = Client::new("other-synthetic-token").unwrap();
    assert!(matches!(
        attempt.pending_request(other.write_scope()),
        Err(Error::WrongConnection)
    ));
}

#[test]
fn malformed_envelopes_and_unsupported_mutation_fields_do_not_become_recovery_records() {
    let client = Client::new("synthetic-token").unwrap();
    let attempt = client.capture(create()).unwrap();
    let bytes = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    let changes: Vec<(&str, Value)> = vec![
        ("version", json!(2)),
        ("kind", json!("delete")),
        ("operation", json!("not-a-uuid")),
        ("target", json!(1)),
        ("extra", json!("private unsupported content")),
    ];
    for (name, value) in changes {
        assert!(matches!(
            decode(&mutate(&bytes, |stored| stored[name] = value), &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    for field in [
        "bypassPeriodClosuresAndApprovals",
        "idempotencyKey",
        "unknown",
    ] {
        let bad = mutate(&bytes, |stored| {
            let mut body: Value = serde_json::from_str(stored["body"].as_str().unwrap()).unwrap();
            body[field] = json!(true);
            stored["body"] = Value::String(serde_json::to_string(&body).unwrap());
        });
        assert!(matches!(
            decode(&bad, &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    let mut duplicate = String::from_utf8(bytes.clone()).unwrap();
    duplicate.insert_str(1, "\"version\":1,");
    assert!(matches!(
        decode(duplicate.as_bytes(), &context()),
        Err(Error::InvalidRecovery)
    ));
    assert!(matches!(
        decode(&vec![0; MAX_RECOVERY_BYTES + 1], &context()),
        Err(Error::InvalidRecovery)
    ));
    assert!(
        !RecoveryError::Domain(Error::InvalidRecovery)
            .to_string()
            .contains("private unsupported content")
    );
}

#[test]
fn update_target_baseline_and_payload_are_required_and_strictly_scoped() {
    let client = Client::new("synthetic-token").unwrap();
    let attempt = client.capture(Draft::edit(&row()).unwrap()).unwrap();
    let bytes = encode(
        attempt.pending_request(client.write_scope()).unwrap(),
        &context(),
    )
    .unwrap();
    for bad in [
        mutate(&bytes, |v| v["baseline"] = Value::Null),
        mutate(&bytes, |v| v["target"] = json!(999)),
        mutate(&bytes, |v| v["baseline"]["unknown"] = json!(true)),
        mutate(&bytes, |v| {
            v["baseline"]["attributes"]["values"] = Value::Null
        }),
    ] {
        assert!(matches!(
            decode(&bad, &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    let bad = mutate(&bytes, |stored| {
        let mut body: Value = serde_json::from_str(stored["body"].as_str().unwrap()).unwrap();
        body["issueId"] = json!(456);
        stored["body"] = Value::String(serde_json::to_string(&body).unwrap());
    });
    assert!(matches!(
        decode(&bad, &context()),
        Err(Error::InvalidRecovery)
    ));
}

#[test]
fn terminal_encoding_requires_a_scoped_observed_outcome_and_preserves_large_ids() {
    let client = Client::new("synthetic-token").unwrap();
    let original = row();
    for confirmed in [false, true] {
        for draft in [
            create(),
            Draft::edit(&original).unwrap(),
            Draft::delete(&original).unwrap(),
        ] {
            let mut attempt = client.capture(draft).unwrap();
            assert!(matches!(
                attempt.terminal_request(client.write_scope()),
                Err(Error::WriteBlocked)
            ));
            assert!(matches!(
                Terminal::capture(&attempt),
                Err(Error::WriteBlocked)
            ));
            let request = attempt.begin(client.write_scope()).unwrap();
            assert!(matches!(
                attempt.terminal_request(client.write_scope()),
                Err(Error::WriteBlocked)
            ));
            let id = request
                .draft()
                .worklog_id()
                .unwrap_or(WorklogId::parse(9007199254740993).unwrap());
            let outcome = if confirmed {
                WriteOutcome::Confirmed(id)
            } else {
                WriteOutcome::Rejected
            };
            assert!(attempt.observe(request.operation_id(), outcome));
            assert!(matches!(
                attempt.terminal_request(super::super::WriteScope::new()),
                Err(Error::WrongConnection)
            ));
            let bytes = encode_with_outcome(
                attempt.terminal_request(client.write_scope()).unwrap(),
                &context(),
                Some(Terminal::capture(&attempt).unwrap()),
            )
            .unwrap();
            let restored = decode(&bytes, &context()).unwrap();
            assert_eq!(restored.phase(), attempt.phase());
            assert!(restored.confirmed_id() == if confirmed { Some(id) } else { None });
            assert_eq!(restored.original_body(), request.body());
            assert_eq!(restored.operation_id(), request.operation_id());
            assert!(matches!(
                attempt.begin(client.write_scope()),
                Err(Error::WriteBlocked)
            ));
            for field in ["profile", "origin", "account"] {
                assert!(matches!(
                    decode(
                        &mutate(&bytes, |value| value[field] = json!("another-identity")),
                        &context()
                    ),
                    Err(Error::WrongConnection)
                ));
            }
        }
    }
}

#[test]
fn malformed_terminal_outcomes_and_interrupted_records_never_allow_recovery() {
    let client = Client::new("synthetic-token").unwrap();
    let attempt = client.capture(Draft::delete(&row()).unwrap()).unwrap();
    let request = attempt.pending_request(client.write_scope()).unwrap();
    let pending = encode(request, &context()).unwrap();
    for outcome in [
        json!({"status":"confirmed","worklog_id":0}),
        json!({"status":"confirmed","worklog_id":124}),
        json!({"status":"confirmed","worklog_id":"123"}),
        json!({"status":"confirmed","worklog_id":123.5}),
        json!({"status":"confirmed"}),
        json!({"status":"confirmed","worklog_id":123,"retry":true}),
        json!({"status":"rejected","worklog_id":123}),
        json!({"status":"ready"}),
        json!({"status":"unconfirmed"}),
        json!(false),
    ] {
        assert!(matches!(
            decode(
                &mutate(&pending, |value| value["outcome"] = outcome),
                &context()
            ),
            Err(Error::InvalidRecovery)
        ));
    }
    let bytes = mutate(&pending, |value| {
        value["outcome"] = json!({"status":"confirmed","worklog_id":123})
    });
    for end in 1..bytes.len() {
        assert!(matches!(
            decode(&bytes[..end], &context()),
            Err(Error::InvalidRecovery)
        ));
    }
    let mut duplicate = String::from_utf8(bytes).unwrap();
    duplicate.insert_str(1, "\"outcome\":{\"status\":\"rejected\"},");
    assert!(matches!(
        decode(duplicate.as_bytes(), &context()),
        Err(Error::InvalidRecovery)
    ));
}

#[test]
fn an_uncertain_or_mismatched_outcome_cannot_be_encoded_as_terminal() {
    let client = Client::new("synthetic-token").unwrap();
    let mut uncertain = client.capture(create()).unwrap();
    let request = uncertain.begin(client.write_scope()).unwrap();
    assert!(uncertain.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    assert!(matches!(
        uncertain.terminal_request(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    assert!(matches!(
        Terminal::capture(&uncertain),
        Err(Error::WriteBlocked)
    ));
    for draft in [Draft::edit(&row()).unwrap(), Draft::delete(&row()).unwrap()] {
        let mut attempt = client.capture(draft).unwrap();
        let request = attempt.begin(client.write_scope()).unwrap();
        assert!(attempt.observe(
            request.operation_id(),
            WriteOutcome::Confirmed(WorklogId::parse(999).unwrap())
        ));
        assert!(matches!(
            encode_with_outcome(
                attempt.terminal_request(client.write_scope()).unwrap(),
                &context(),
                Some(Terminal::capture(&attempt).unwrap())
            ),
            Err(Error::InvalidRecovery)
        ));
    }
}

#[tokio::test]
async fn terminal_checkpoint_rechecks_live_identity_without_writes_or_authentication_fallback() {
    use crate::tempo::jira::tests::{Fixture, reply};
    let fixture = Fixture::new(vec![
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(200, r#"{"accountId":"synthetic-user"}"#),
        reply(401, "private synthetic error body"),
        reply(200, r#"{"accountId":"another-user"}"#),
    ]);
    let jira = JiraClient::build(
        fixture.base.clone(),
        "synthetic@example.invalid",
        "synthetic-token",
        false,
    )
    .unwrap();
    let config = Path::new(if cfg!(windows) {
        "D:/synthetic-unused/tempo-terminal-recovery.json"
    } else {
        "/synthetic-unused/tempo-terminal-recovery.json"
    });
    let client = Client::new("synthetic-tempo-token").unwrap();
    let mut attempt = client.capture(create()).unwrap();
    assert!(matches!(
        client.terminal_checkpoint(&attempt, config, &jira).await,
        Err(RecoveryError::Domain(Error::WriteBlocked))
    ));
    assert!(fixture.requests.lock().unwrap().is_empty());
    let request = attempt.begin(client.write_scope()).unwrap();
    let id = WorklogId::parse(9007199254740993).unwrap();
    assert!(attempt.observe(request.operation_id(), WriteOutcome::Confirmed(id)));
    let other = Client::new("another-synthetic-tempo-token").unwrap();
    assert!(matches!(
        other.terminal_checkpoint(&attempt, config, &jira).await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert!(fixture.requests.lock().unwrap().is_empty());
    let bytes = client
        .terminal_checkpoint(&attempt, config, &jira)
        .await
        .unwrap();
    let restored = inspect(&bytes, config, &jira).await.unwrap();
    assert_eq!(restored.phase(), WritePhase::Confirmed);
    assert!(restored.confirmed_id() == Some(id));
    assert_eq!(restored.operation_id(), request.operation_id());
    assert_eq!(restored.original_body(), request.body());
    assert!(matches!(
        client.terminal_checkpoint(&attempt, config, &jira).await,
        Err(RecoveryError::Identity(JiraError::Authentication))
    ));
    assert!(matches!(
        client.terminal_checkpoint(&attempt, config, &jira).await,
        Err(RecoveryError::Domain(Error::WrongConnection))
    ));
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    assert!(attempt.confirmed_id() == Some(id));
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with("GET /rest/api/3/myself HTTP/1.1\r\n"))
    );
}
