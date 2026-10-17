use super::*;
use crate::todoist::{
    change::{ChangeError, SavedChange},
    storage::PendingFile,
};

fn command(request: &Captured) -> Value {
    let value = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "commands")
        .unwrap()
        .1
        .into_owned();
    serde_json::from_str::<Value>(&value).unwrap()[0].clone()
}

fn profile() -> Response {
    Response::json(json!({"id":"synthetic-user","tz_info":{}}))
}

#[tokio::test]
async fn durable_confirmed_and_rejected_outcomes_reopen_for_cleanup_without_a_post() {
    for rejected in [false, true] {
        let disk = DiskRecovery::new();
        let (client, worker, captures) = server(3, move |request, index| {
            if index != 1 {
                assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
                return Some(profile());
            }
            assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
            let command = command(request);
            let id = command["uuid"].as_str().unwrap();
            Some(Response::json(if rejected {
                json!({"sync_status":{id:{"error_code":400,"error":"Synthetic rejection"}}})
            } else {
                json!({"sync_status":{id:"ok"}})
            }))
        });
        let mut attempt = WriteAttempt::new(
            WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap(),
        );
        let original = client.checkpoint(&attempt, &disk.config()).await.unwrap();
        let mut file = PendingFile::open(&disk.config()).unwrap();
        file.persist(&original).unwrap();
        let request = attempt.begin(client.write_scope()).unwrap();
        let reply = client.send(&request).await.unwrap();
        assert!(attempt.observe(request.command_id(), reply.outcome));
        let marker = attempt.acknowledgement(&original).unwrap().unwrap();
        file.append_acknowledgement(&marker).unwrap();
        let stored = std::fs::read(disk.pending()).unwrap();
        assert!(stored.starts_with(&original));
        assert_eq!(stored[original.len()], b'{');
        drop(file);
        drop(attempt);
        let reconnected = Client::build("synthetic-token", client.base.clone(), false).unwrap();
        let mut recovered = SavedChange::recover(&reconnected, &disk.config())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered.phase(),
            if rejected {
                WritePhase::Rejected
            } else {
                WritePhase::Confirmed
            }
        );
        assert!(matches!(
            recovered.dispatch(&reconnected).await,
            Err(ChangeError::Transport(TransportError::Domain(
                Error::WriteBlocked
            )))
        ));
        assert_eq!(std::fs::read(disk.pending()).unwrap(), stored);
        recovered.retry_cleanup().unwrap();
        assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), 3);
        assert_eq!(
            captures
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.line.starts_with("POST "))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn interrupted_or_mismatched_outcome_records_are_preserved_without_replay() {
    for invalid in 0..6 {
        let disk = DiskRecovery::new();
        let (client, worker, captures) = server(2, |request, _| {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            Some(profile())
        });
        let mut attempt = WriteAttempt::new(
            WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap(),
        );
        let original = client.checkpoint(&attempt, &disk.config()).await.unwrap();
        let request = attempt.begin(client.write_scope()).unwrap();
        assert!(attempt.observe(
            request.command_id(),
            WriteOutcome::Confirmed(Id::parse("task").unwrap())
        ));
        let marker = attempt.acknowledgement(&original).unwrap().unwrap();
        let mut value: Value = serde_json::from_slice(&marker).unwrap();
        match invalid {
            0 => value["command_id"] = json!(uuid::Uuid::new_v4().to_string()),
            1 => value["checkpoint_sha256"] = json!("0".repeat(64)),
            2 => value["outcome"]["confirmed"]["task"] = json!("different-task"),
            3 => value["unexpected"] = json!(true),
            _ => {}
        }
        let marker = if invalid == 5 {
            b"{".to_vec()
        } else if invalid == 4 {
            b"{\"version\":".to_vec()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        let mut file = PendingFile::open(&disk.config()).unwrap();
        file.persist(&original).unwrap();
        file.append_acknowledgement(&marker).unwrap();
        let stored = std::fs::read(disk.pending()).unwrap();
        assert!(matches!(
            SavedChange::recover(&client, &disk.config()).await,
            Err(ChangeError::Transport(TransportError::Domain(
                Error::InvalidRecovery
            )))
        ));
        assert_eq!(std::fs::read(disk.pending()).unwrap(), stored);
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), 2);
        assert!(
            captures
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.line.starts_with("GET "))
        );
    }
}

#[cfg(windows)]
#[tokio::test]
async fn real_delete_sharing_denial_preserves_a_durable_acknowledgement_across_restart() {
    use std::os::windows::fs::OpenOptionsExt;
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(4, |request, index| {
        if index != 2 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            return Some(profile());
        }
        let command = command(request);
        let id = command["uuid"].as_str().unwrap();
        Some(Response::json(json!({"sync_status":{id:"ok"}})))
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let mut change = SavedChange::capture(&client, &disk.config(), request)
        .await
        .unwrap();
    let original = std::fs::read(disk.pending()).unwrap();
    // Permit reads and writes but refuse deletion of this exact owned file.
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(disk.pending())
        .unwrap();
    let report = change.dispatch(&client).await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert!(report.acknowledgement_durable);
    assert!(report.cleanup_problem.is_some());
    assert!(std::fs::read(disk.pending()).unwrap().len() > original.len());
    drop(change);
    let reconnected = Client::build("synthetic-token", client.base.clone(), false).unwrap();
    let mut recovered = SavedChange::recover(&reconnected, &disk.config())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.phase(), WritePhase::Confirmed);
    assert!(recovered.acknowledgement_durable());
    assert!(recovered.dispatch(&reconnected).await.is_err());
    drop(blocker);
    recovered.retry_cleanup().unwrap();
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 4);
    assert_eq!(
        captures
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.line.starts_with("POST "))
            .count(),
        1
    );
}

#[cfg(windows)]
#[tokio::test]
async fn denied_writes_and_deletion_require_keep_open_then_local_cleanup_only() {
    use std::os::windows::fs::OpenOptionsExt;
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(3, |request, index| {
        if index != 2 {
            return Some(profile());
        }
        let id = command(request)["uuid"].as_str().unwrap().to_owned();
        Some(Response::json(json!({"sync_status":{id:"ok"}})))
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let mut change = SavedChange::capture(&client, &disk.config(), request)
        .await
        .unwrap();
    let original = std::fs::read(disk.pending()).unwrap();
    let pending_path = disk.pending();
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(disk.pending())
        .unwrap();
    let report = change.dispatch(&client).await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert!(!report.acknowledgement_durable);
    assert!(report.cleanup_problem.is_some());
    assert_eq!(std::fs::read(disk.pending()).unwrap(), original);
    assert!(change.dispatch(&client).await.is_err());
    drop(blocker);
    change.retry_cleanup().unwrap();
    assert!(change.acknowledgement_durable());
    assert!(!pending_path.exists());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 3);
    assert_eq!(
        captures
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.line.starts_with("POST "))
            .count(),
        1
    );
}
