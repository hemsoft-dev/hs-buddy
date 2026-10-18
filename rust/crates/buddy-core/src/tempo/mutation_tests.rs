use serde_json::{Value, json};

use super::*;
use crate::tempo::{AccountId, Client, IssueId};

fn client() -> Client {
    Client::new("synthetic-tempo-token").unwrap()
}
fn draft() -> Draft {
    Draft::create(
        AccountId::parse("synthetic-user").unwrap(),
        IssueId::parse(124).unwrap(),
        "2026-10-16".parse().unwrap(),
        600,
    )
    .unwrap()
}
fn row() -> Value {
    json!({"tempoWorklogId":1,"self":"https://api.tempo.io/4/worklogs/1","issue":{"id":124,"self":"https://api.tempo.io/4/issues/124"},"author":{"accountId":"synthetic-user"},
        "timeSpentSeconds":600,"startDate":"2026-10-16","createdAt":"2026-10-16T01:00:00Z", "updatedAt":"2026-10-16T02:00:00Z",
        "attributes":{"self":"https://api.tempo.io/4/worklogs/1/work-attribute-values","values":[]}})
}

#[test]
fn uncertain_dispatch_preserves_frozen_payload_and_never_offers_an_idempotent_resend() {
    let client = client();
    let mut attempt = client.capture(draft()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    let original = request.body().to_vec();
    assert_eq!(request.operation_id().get_version_num(), 4);
    assert!(
        !serde_json::from_slice::<Value>(&original)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("idempotencyKey")
    );
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    assert!(attempt.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
    assert!(!attempt.can_replace());
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    assert!(attempt.confirmed_id().is_none());
    assert!(request.body() == original);
}

#[test]
fn another_connection_or_stale_worker_cannot_dispatch_or_complete_the_change() {
    let client = client();
    let mut attempt = client.capture(draft()).unwrap();
    assert!(matches!(
        attempt.begin(WriteScope::new()),
        Err(Error::WrongConnection)
    ));
    assert_eq!(attempt.phase(), WritePhase::Ready);
    let request = attempt.begin(client.write_scope()).unwrap();
    assert!(!attempt.observe(Uuid::new_v4(), WriteOutcome::Rejected));
    assert_eq!(attempt.phase(), WritePhase::InFlight);
    assert!(attempt.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    assert!(!attempt.observe(
        request.operation_id(),
        WriteOutcome::Confirmed(WorklogId::parse(1).unwrap())
    ));
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
}

#[test]
fn an_acknowledged_change_stays_acknowledged_when_refresh_or_a_late_worker_fails() {
    let client = client();
    let mut attempt = client.capture(draft()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    let outcome = request.decode_success(200, &serde_json::to_vec(&row()).unwrap());
    assert!(attempt.observe(request.operation_id(), outcome));
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    assert_eq!(attempt.confirmed_id().unwrap().get(), 1);
    assert!(!attempt.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    assert!(attempt.can_replace());
}

#[test]
fn unsupported_or_foreign_success_responses_remain_uncertain_for_create_and_update() {
    let client = client();
    let mut attempt = client.capture(draft()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    let mut wrong_author = row();
    wrong_author["author"]["accountId"] = json!("other-user");
    let mut wrong_issue = row();
    wrong_issue["issue"]["id"] = json!(125);
    let mut missing_issue = row();
    missing_issue.as_object_mut().unwrap().remove("issue");
    for invalid in [wrong_author, wrong_issue, missing_issue, json!({})] {
        assert!(matches!(
            request.decode_success(200, &serde_json::to_vec(&invalid).unwrap()),
            WriteOutcome::Unconfirmed
        ));
    }
    let baseline = parse_worklog(&serde_json::to_vec(&row()).unwrap()).unwrap();
    let mut attempt = client.capture(Draft::edit(&baseline).unwrap()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    let mut wrong_id = row();
    wrong_id["tempoWorklogId"] = json!(2);
    assert!(matches!(
        request.decode_success(200, &serde_json::to_vec(&wrong_id).unwrap()),
        WriteOutcome::Unconfirmed
    ));
    assert!(matches!(
        request.decode_success(200, b"private unsupported body"),
        WriteOutcome::Unconfirmed
    ));
    assert!(matches!(
        request.decode_success(200, &vec![b' '; crate::tempo::MAX_RESPONSE_BYTES + 1]),
        WriteOutcome::Unconfirmed
    ));
    assert!(
        matches!(request.decode_success(200, &serde_json::to_vec(&row()).unwrap()), WriteOutcome::Confirmed(id) if id.get() == 1)
    );
}

#[test]
fn deletion_requires_its_204_empty_acknowledgement_and_confirms_only_the_captured_id() {
    let client = client();
    let original = parse_worklog(&serde_json::to_vec(&row()).unwrap()).unwrap();
    let mut attempt = client.capture(Draft::delete(&original).unwrap()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    assert!(request.body().is_empty());
    for (status, bytes) in [
        (200, &b""[..]),
        (201, &b""[..]),
        (404, &b""[..]),
        (204, &b"{}"[..]),
    ] {
        assert!(matches!(
            request.decode_success(status, bytes),
            WriteOutcome::Unconfirmed
        ));
    }
    let outcome = request.decode_success(204, b"");
    assert!(attempt.observe(request.operation_id(), outcome));
    assert!(attempt.confirmed_id() == Some(original.id));
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
}

#[test]
fn an_uncertain_deletion_cannot_be_dispatched_again_or_replaced() {
    let client = client();
    let original = parse_worklog(&serde_json::to_vec(&row()).unwrap()).unwrap();
    let mut attempt = client.capture(Draft::delete(&original).unwrap()).unwrap();
    let request = attempt.begin(client.write_scope()).unwrap();
    assert!(attempt.observe(request.operation_id(), WriteOutcome::Unconfirmed));
    assert!(!attempt.can_replace());
    assert!(matches!(
        attempt.begin(client.write_scope()),
        Err(Error::WriteBlocked)
    ));
    assert!(request.body().is_empty());
    assert!(request.draft().worklog_id() == Some(original.id));
}

#[test]
fn create_and_update_require_the_documented_record_response_status() {
    let client = client();
    let original = parse_worklog(&serde_json::to_vec(&row()).unwrap()).unwrap();
    for draft in [draft(), Draft::edit(&original).unwrap()] {
        let mut attempt = client.capture(draft).unwrap();
        let request = attempt.begin(client.write_scope()).unwrap();
        let bytes = serde_json::to_vec(&row()).unwrap();
        for status in [201, 204, 299, 400, 500] {
            assert!(matches!(
                request.decode_success(status, &bytes),
                WriteOutcome::Unconfirmed
            ));
        }
        assert!(matches!(
            request.decode_success(200, &bytes),
            WriteOutcome::Confirmed(_)
        ));
    }
}
