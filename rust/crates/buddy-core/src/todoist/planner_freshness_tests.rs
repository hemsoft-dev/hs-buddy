use super::*;

#[tokio::test]
async fn acknowledged_completion_with_failed_refresh_cannot_complete_the_stale_row_twice() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(9, |request, index| {
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
            Some(Response::json(json!({"sync_status":{uuid:"ok"}})))
        } else if index == 8 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            Some(Response {
                status: 503,
                body: String::new(),
                headers: String::new(),
            })
        } else {
            Some(loaded(request))
        }
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    let original = planner.refresh().await.unwrap();
    assert!(planner.can_submit());
    let report = planner
        .submit(TaskChange::Complete(Id::parse("task").unwrap()))
        .await
        .unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert!(!planner.can_submit());
    assert!(planner.pending().is_none());
    assert!(planner.refresh().await.is_err());
    assert!(Arc::ptr_eq(planner.data().unwrap(), &original));
    assert!(!planner.can_submit());
    assert!(matches!(
        planner
            .submit(TaskChange::Complete(Id::parse("task").unwrap()))
            .await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
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
