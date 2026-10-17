use super::*;
use crate::todoist::{
    Error,
    change::ChangeError,
    planner::{Planner, TaskChange},
};
#[path = "planner_access_tests.rs"]
mod access;
#[path = "planner_freshness_tests.rs"]
mod freshness;

fn profile() -> Response {
    Response::json(json!({"id":"planner-account","tz_info":{"timezone":"America/New_York"}}))
}

fn projects() -> Response {
    Response::json(
        json!({"results":[{"id":"project","name":"Synthetic project","color":"blue","parent_id":null,"child_order":0}],"next_cursor":null}),
    )
}

fn loaded(request: &Captured) -> Response {
    if request.line == "GET /api/v1/user HTTP/1.1" {
        profile()
    } else if request.line.starts_with("GET /api/v1/projects?") {
        projects()
    } else {
        assert!(request.line.starts_with("GET /api/v1/tasks?"));
        Response::json(json!({"results":[task("task")],"next_cursor":null}))
    }
}

#[tokio::test]
async fn unavailable_project_cannot_be_retargeted_to_inbox_or_publish_a_change() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(5, |request, _| Some(loaded(request)));
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    let draft = CreateDraft {
        content: "Synthetic task in a removed project".into(),
        description: String::new(),
        project: Some(Id::parse("removed-project").unwrap()),
        priority: Priority::from_api(1).unwrap(),
        due: None,
    };
    assert!(matches!(
        planner.submit(TaskChange::Create(draft)).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::InvalidDraft
        )))
    ));
    assert!(planner.can_submit());
    assert!(planner.pending().is_none() && !planner.recovery_blocked());
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 5);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
}

#[tokio::test]
async fn failed_refresh_keeps_complete_account_data_and_never_posts() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(8, |request, index| {
        if index == 7 {
            assert!(request.line.starts_with("GET /api/v1/tasks?"));
            return Some(Response {
                status: 503,
                body: String::new(),
                headers: String::new(),
            });
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    let original = planner.refresh().await.unwrap();
    assert_eq!(original.tasks[0].id.as_str(), "task");
    assert_eq!(original.projects[0].name, "Synthetic project");
    assert_eq!(original.account.as_str(), "planner-account");
    assert_eq!(original.calendar.zone_name(), "America/New_York");
    assert!(matches!(
        planner.refresh().await,
        Err(TransportError::Service)
    ));
    assert!(Arc::ptr_eq(planner.data().unwrap(), &original));
    assert!(planner.pending().is_none());
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 8);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
}

#[tokio::test]
async fn account_change_during_read_cannot_publish_mixed_data() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(5, |request, index| {
        if index == 4 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            Some(Response::json(
                json!({"id":"foreign-account","tz_info":{"timezone":"America/New_York"}}),
            ))
        } else {
            Some(loaded(request))
        }
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    assert!(matches!(
        planner.refresh().await,
        Err(TransportError::Domain(Error::WrongConnection))
    ));
    assert!(planner.data().is_none());
    assert_eq!(planner.account().as_str(), "planner-account");
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn lost_reply_refresh_and_explicit_retry_preserve_one_uuid_and_disk_snapshot() {
    let disk = DiskRecovery::new();
    let mut sent = None;
    let (client, worker, captures) = server(14, move |request, index| {
        if matches!(index, 7 | 13) {
            assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
            let commands = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "commands")
                .unwrap()
                .1
                .into_owned();
            if index == 7 {
                sent = Some(commands);
                None
            } else {
                assert_eq!(sent.as_ref().unwrap(), &commands);
                let parsed: Value = serde_json::from_str(&commands).unwrap();
                let uuid = parsed[0]["uuid"].as_str().unwrap();
                Some(Response::json(json!({"sync_status":{uuid:"ok"}})))
            }
        } else {
            Some(loaded(request))
        }
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    let report = planner
        .submit(TaskChange::Complete(Id::parse("task").unwrap()))
        .await
        .unwrap();
    assert_eq!(report.phase, WritePhase::Unconfirmed);
    let original = std::fs::read(disk.pending()).unwrap();
    let uuid = planner.pending().unwrap().request().command_id();
    assert!(matches!(
        planner
            .submit(TaskChange::Delete(Id::parse("task").unwrap()))
            .await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
    planner.refresh().await.unwrap();
    assert_eq!(planner.pending().unwrap().request().command_id(), uuid);
    assert_eq!(std::fs::read(disk.pending()).unwrap(), original);
    let report = planner.retry().await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert_eq!(report.task.unwrap().as_str(), "task");
    assert!(planner.pending().is_none());
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 14);
    assert_eq!(
        captures
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.line.starts_with("POST "))
            .count(),
        2
    );
}

#[tokio::test]
async fn unconfirmed_change_reopens_without_resend_and_wrong_account_reconnect_preserves_it() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(10, |request, index| {
        if index == 7 {
            assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
            None
        } else {
            Some(loaded(request))
        }
    });
    let base = client.base.clone();
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    planner
        .submit(TaskChange::Delete(Id::parse("task").unwrap()))
        .await
        .unwrap();
    let original = std::fs::read(disk.pending()).unwrap();
    let uuid = planner.pending().unwrap().request().command_id();
    drop(planner);
    let client = Client::build("synthetic-token", base, false).unwrap();
    let mut reopened = Planner::connect(client, disk.config(), None).await.unwrap();
    assert_eq!(reopened.pending().unwrap().phase(), WritePhase::Unconfirmed);
    assert_eq!(reopened.pending().unwrap().request().command_id(), uuid);
    let (foreign, foreign_worker, foreign_captures) = server(1, |request, _| {
        assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
        Some(Response::json(
            json!({"id":"foreign","tz_info":{"timezone":"UTC"}}),
        ))
    });
    assert!(matches!(
        reopened.reconnect(foreign).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert_eq!(reopened.pending().unwrap().request().command_id(), uuid);
    assert_eq!(std::fs::read(disk.pending()).unwrap(), original);
    assert_eq!(reopened.account().as_str(), "planner-account");
    worker.join().unwrap();
    foreign_worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 10);
    assert_eq!(foreign_captures.lock().unwrap().len(), 1);
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

#[tokio::test]
async fn acknowledged_write_retains_its_outcome_when_cleanup_fails_and_refresh_never_replays() {
    use crate::todoist::storage::StorageError;
    let disk = DiskRecovery::new();
    let directory = disk.0.clone();
    let (client, worker, captures) = server(12, move |request, _| {
        if request.line.starts_with("POST ") {
            let commands = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "commands")
                .unwrap()
                .1
                .into_owned();
            let value: Value = serde_json::from_str(&commands).unwrap();
            let uuid = value[0]["uuid"].as_str().unwrap();
            let path = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|value| value == "json"))
                .unwrap();
            std::fs::write(path, b"changed-by-another-window").unwrap();
            Some(Response::json(json!({"sync_status":{uuid:"ok"}})))
        } else {
            Some(loaded(request))
        }
    });
    let base = client.base.clone();
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    let report = planner
        .submit(TaskChange::Complete(Id::parse("task").unwrap()))
        .await
        .unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert_eq!(report.cleanup_problem, Some(StorageError::Changed));
    assert_eq!(report.task.unwrap().as_str(), "task");
    planner.refresh().await.unwrap();
    assert_eq!(planner.pending().unwrap().phase(), WritePhase::Confirmed);
    assert!(matches!(
        planner.retry().await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
    assert_eq!(
        planner.retry_cleanup().unwrap_err(),
        ChangeError::Storage(StorageError::Changed)
    );
    let replacement = Client::build("synthetic-token", base, false).unwrap();
    assert!(matches!(
        planner.reconnect(replacement).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
    assert_eq!(planner.pending().unwrap().phase(), WritePhase::Confirmed);
    assert_eq!(
        std::fs::read(disk.pending()).unwrap(),
        b"changed-by-another-window"
    );
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 12);
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

#[tokio::test]
async fn failed_capture_blocks_draft_replacement_and_recovery_preserves_unknown_file() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(6, |request, _| Some(loaded(request)));
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    let mut file = crate::todoist::storage::PendingFile::open(&disk.config()).unwrap();
    file.persist(b"unknown-recovery").unwrap();
    assert!(
        planner
            .submit(TaskChange::Complete(Id::parse("task").unwrap()))
            .await
            .is_err()
    );
    assert!(planner.recovery_blocked());
    assert!(matches!(
        planner
            .submit(TaskChange::Delete(Id::parse("task").unwrap()))
            .await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
    assert!(planner.check_recovery().await.is_err());
    assert!(planner.recovery_blocked());
    assert_eq!(std::fs::read(disk.pending()).unwrap(), b"unknown-recovery");
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 6);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
}

#[tokio::test]
async fn missing_record_cannot_clear_an_unconfirmed_change_by_reconnecting() {
    use crate::todoist::storage::StorageError;
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(9, |request, _| {
        if request.line.starts_with("POST ") {
            None
        } else {
            Some(loaded(request))
        }
    });
    let base = client.base.clone();
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    planner
        .submit(TaskChange::Complete(Id::parse("task").unwrap()))
        .await
        .unwrap();
    let uuid = planner.pending().unwrap().request().command_id();
    std::fs::remove_file(disk.pending()).unwrap();
    let replacement = Client::build("synthetic-token", base, false).unwrap();
    assert_eq!(
        planner.reconnect(replacement).await.unwrap_err(),
        ChangeError::Storage(StorageError::Changed)
    );
    assert_eq!(planner.pending().unwrap().phase(), WritePhase::Unconfirmed);
    assert_eq!(planner.pending().unwrap().request().command_id(), uuid);
    assert!(planner.data().is_some());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 9);
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
