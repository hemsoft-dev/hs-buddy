use super::*;
use crate::copilot_runs::Context;

#[test]
fn history_filters_keep_query_arguments_bounded_and_typed() {
    let (name, args) = history_request(
        &HistoryFilter::Category("pr-review".into()),
        Some("opaque-cursor"),
        10,
    )
    .unwrap();
    assert_eq!(name, "copilotNative:listResultsByCategory");
    assert_eq!(
        args,
        json!({"category":"pr-review","paginationOpts":{"numItems":10,"cursor":"opaque-cursor"}})
    );
    assert_eq!(
        history_request(
            &HistoryFilter::Status(crate::copilot_results::Status::Failed),
            None,
            1
        )
        .unwrap()
        .1["status"],
        "failed"
    );
    assert_eq!(
        history_request(&HistoryFilter::All, None, 25).unwrap().0,
        "copilotNative:listResults"
    );
    for filter in [
        HistoryFilter::Status(crate::copilot_results::Status::Unknown),
        HistoryFilter::Category(String::new()),
        HistoryFilter::Category("bad\ncategory".into()),
    ] {
        assert_eq!(history_request(&filter, None, 10), Err(Error::Rejected));
    }
    assert_eq!(
        history_request(&HistoryFilter::All, None, 0),
        Err(Error::Rejected)
    );
    assert_eq!(
        history_request(&HistoryFilter::All, Some(&"x".repeat(16 * 1024 + 1)), 10),
        Err(Error::Rejected)
    );
}

fn execution(review: Option<ReviewContext>, saved: bool) -> Execution {
    Execution::new(
        Context {
            account: "synthetic-account".into(),
            model: "gpt-4.1".into(),
            review,
        },
        "Private prompt 界".into(),
        saved,
    )
    .unwrap()
}

#[test]
fn immutable_requests_bind_the_validated_pr_snapshot() {
    let context = ReviewContext {
        repository: "owner/repo".into(),
        pull_request: 7,
        head: "a".repeat(40),
    };
    let execution = execution(Some(context.clone()), true);
    assert!(NativeRequest::new(&execution, None).is_err());
    let snapshot = ReviewSnapshot {
        account: "synthetic-account".into(),
        context,
        title: "Synthetic title".into(),
        threads: ThreadStats {
            total: 2,
            unresolved: 1,
            outdated: 0,
        },
    };
    let request = NativeRequest::new(&execution, Some(&snapshot)).unwrap();
    let wrong_account = ReviewSnapshot {
        account: "other-account".into(),
        context: snapshot.context.clone(),
        title: snapshot.title.clone(),
        threads: snapshot.threads,
    };
    assert!(matches!(
        NativeRequest::new(&execution, Some(&wrong_account)),
        Err(Failure::InvalidContext)
    ));
    assert_eq!(
        uuid::Uuid::parse_str(request.identity())
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(request.arguments["prompt"], "Private prompt 界");
    assert_eq!(request.arguments["review"]["head"], "a".repeat(40));
    assert_eq!(request.arguments["review"]["threadStats"]["total"], 2);
    assert_eq!(
        request.identity(),
        request.arguments["requestId"].as_str().unwrap()
    );
}

#[test]
fn unsaved_or_already_started_execution_cannot_create_another_record() {
    assert!(NativeRequest::new(&execution(None, false), None).is_err());
    let mut execution = execution(None, true);
    assert!(execution.record_created(execution.key(), "result-1".into()));
    assert!(NativeRequest::new(&execution, None).is_err());
}

#[test]
fn malformed_or_conflicting_creation_receipts_are_rejected() {
    for value in [
        json!(null),
        json!({"id":"","created":true,"status":"pending"}),
        json!({"id":"a","created":true,"status":"running"}),
        json!({"id":"a","created":true,"status":"future"}),
        json!({"id":"a\n","created":false,"status":"pending"}),
    ] {
        assert!(matches!(begin_receipt(value), Err(Error::InvalidResponse)));
    }
    for status in ["pending", "running", "completed", "failed", "deleted"] {
        let receipt =
            begin_receipt(json!({"id":"result-1","created":false,"status":status})).unwrap();
        assert!(!receipt.created);
    }
    assert!(
        begin_receipt(json!({"id":"result-1","created":true,"status":"pending"}))
            .unwrap()
            .created
    );
}

#[test]
fn only_acknowledged_running_claims_can_authorize_sending() {
    assert_eq!(
        claim_receipt(json!({"claimed":true,"status":"running"})).unwrap(),
        ClaimReceipt {
            claimed: true,
            status: RecordStatus::Running
        }
    );
    assert!(
        !claim_receipt(json!({"claimed":false,"status":"completed"}))
            .unwrap()
            .claimed
    );
    for value in [
        json!({"claimed":true,"status":"pending"}),
        json!({"claimed":"true","status":"running"}),
        json!(null),
    ] {
        assert_eq!(claim_receipt(value), Err(Error::UnknownWrite));
    }
}

#[test]
fn storage_acknowledgement_requires_matching_identity_status_and_boolean() {
    assert_eq!(
        finish_receipt(
            &json!({"id":"r","status":"completed","saved":true}),
            "r",
            false
        ),
        Ok(())
    );
    assert_eq!(
        finish_receipt(&json!({"id":"r","status":"failed","saved":true}), "r", true),
        Ok(())
    );
    for value in [
        json!({"id":"other","status":"completed","saved":true}),
        json!({"id":"r","status":"failed","saved":true}),
        json!({"id":"r","status":"completed","saved":"true"}),
        json!(null),
    ] {
        assert_eq!(finish_receipt(&value, "r", false), Err(Error::UnknownWrite));
    }
}

#[test]
fn service_errors_cannot_echo_backend_or_private_content() {
    assert_eq!(Error::from(BackendError::UnknownWrite), Error::UnknownWrite);
    assert_eq!(Error::from(BackendError::Rejected), Error::Rejected);
    assert_eq!(Error::from(BackendError::Unavailable), Error::Unavailable);
    assert_eq!(
        failure_code(Failure::AuthenticationRequired),
        "authentication-required"
    );
    assert_eq!(failure_code(Failure::SendUnknown), "delivery-unknown");
    assert_eq!(failure_code(Failure::IncompleteStream), "incomplete-stream");
    assert_eq!(failure_code(Failure::OutputLimit), "output-limit");
    assert_eq!(
        failure_code(Failure::ServiceUnavailable),
        "service-unavailable"
    );
    assert!(CopilotBackend::new("https://private:credential@example.com").is_err());
}
