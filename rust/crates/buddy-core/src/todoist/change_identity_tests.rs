use super::*;
use crate::todoist::change::{ChangeError, SavedChange};
use crate::todoist::storage::{PendingFile, StorageError};

fn profile(request: &Captured, account: &str) -> Option<Response> {
    assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
    Some(Response::json(json!({"id":account,"tz_info":{}})))
}

fn completion(client: &Client) -> WriteRequest {
    WriteRequest::complete(client.write_scope(), &Id::parse("loaded-task").unwrap()).unwrap()
}

#[tokio::test]
async fn loaded_account_mismatch_cannot_publish_recovery_or_send_a_task_change() {
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(1, |request, _| profile(request, "different-account"));
    let result = SavedChange::capture_for_account(
        &client,
        &fixture.config(),
        completion(&client),
        &Id::parse("loaded-account").unwrap(),
    )
    .await;
    assert!(matches!(
        result,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert!(
        PendingFile::open(&fixture.config())
            .unwrap()
            .current()
            .is_none()
    );
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn account_change_after_capture_preserves_original_bytes_without_posting() {
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(2, |request, index| {
        profile(
            request,
            if index == 0 {
                "loaded-account"
            } else {
                "different-account"
            },
        )
    });
    let mut change = SavedChange::capture_for_account(
        &client,
        &fixture.config(),
        completion(&client),
        &Id::parse("loaded-account").unwrap(),
    )
    .await
    .unwrap();
    let path = fixture.pending();
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        change.dispatch(&client).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert_eq!(change.phase(), WritePhase::Ready);
    assert_eq!(std::fs::read(path).unwrap(), original);
    worker.join().unwrap();
    let requests = captures.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
}

#[tokio::test]
async fn recovery_cannot_relabel_another_valid_saved_account_as_the_loaded_account() {
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(2, |request, _| profile(request, "saved-account"));
    let change = SavedChange::capture_for_account(
        &client,
        &fixture.config(),
        completion(&client),
        &Id::parse("saved-account").unwrap(),
    )
    .await
    .unwrap();
    let original = std::fs::read(fixture.pending()).unwrap();
    drop(change);
    assert!(matches!(
        SavedChange::recover_for_account(
            &client,
            &fixture.config(),
            &Id::parse("loaded-account").unwrap()
        )
        .await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert_eq!(std::fs::read(fixture.pending()).unwrap(), original);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn explicit_discard_retires_only_its_record_and_cannot_be_dispatched_afterward() {
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(2, |request, _| profile(request, "loaded-account"));
    let mut change = SavedChange::capture_for_account(
        &client,
        &fixture.config(),
        completion(&client),
        &Id::parse("loaded-account").unwrap(),
    )
    .await
    .unwrap();
    let path = fixture.pending();
    change.discard().unwrap();
    assert!(!path.exists());
    assert!(matches!(
        change.dispatch(&client).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert!(!path.exists());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn explicit_discard_keeps_changed_recovery_bytes_and_performs_no_http() {
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(1, |request, _| profile(request, "loaded-account"));
    let mut change = SavedChange::capture_for_account(
        &client,
        &fixture.config(),
        completion(&client),
        &Id::parse("loaded-account").unwrap(),
    )
    .await
    .unwrap();
    let path = fixture.pending();
    let changed = b"another-window-private-record";
    std::fs::write(&path, changed).unwrap();
    assert_eq!(
        change.discard(),
        Err(ChangeError::Storage(StorageError::Changed))
    );
    assert_eq!(std::fs::read(path).unwrap(), changed);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 1);
}
