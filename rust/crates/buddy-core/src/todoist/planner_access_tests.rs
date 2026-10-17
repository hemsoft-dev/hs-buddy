use super::*;
use crate::todoist::{EditDraft, WriteKind, planner::Coverage};

#[tokio::test]
async fn complete_only_projects_reject_other_commands_before_persist_or_http() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(8, |request, _| {
        if request.line.starts_with("GET /api/v1/projects?") {
            return Some(Response::json(
                json!({"results":[{"id":"project","name":"Complete only","color":"blue","child_order":0,"workspace_id":12345,"role":"complete_only"}],"next_cursor":null}),
            ));
        }
        if request.line.starts_with("POST ") {
            let command = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "commands")
                .unwrap()
                .1
                .into_owned();
            let command: Value = serde_json::from_str(&command).unwrap();
            assert_eq!(command[0]["type"], "item_close");
            let uuid = command[0]["uuid"].as_str().unwrap();
            return Some(Response::json(json!({"sync_status":{uuid:"ok"}})));
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    planner.refresh().await.unwrap();
    let id = Id::parse("task").unwrap();
    assert!(!planner.can_create_in(Some(&Id::parse("project").unwrap())));
    assert!(!planner.can_change_task(&id, WriteKind::Edit));
    assert!(!planner.can_change_task(&id, WriteKind::Delete));
    assert!(planner.can_change_task(&id, WriteKind::Complete));
    for change in [
        TaskChange::Create(CreateDraft {
            content: "Forbidden synthetic task".into(),
            description: String::new(),
            project: Some(Id::parse("project").unwrap()),
            priority: Priority::from_api(1).unwrap(),
            due: None,
        }),
        TaskChange::Edit(
            id.clone(),
            EditDraft {
                content: Some("Forbidden edit".into()),
                description: None,
                priority: None,
                due: None,
            },
        ),
        TaskChange::Delete(id.clone()),
        TaskChange::Reopen(id.clone()),
    ] {
        assert!(matches!(
            planner.submit(change).await,
            Err(ChangeError::Transport(TransportError::Domain(
                Error::WritePermission
            )))
        ));
        assert!(planner.pending().is_none());
        assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    }
    assert_eq!(
        planner
            .submit(TaskChange::Complete(id))
            .await
            .unwrap()
            .phase,
        WritePhase::Confirmed
    );
    worker.join().unwrap();
    let requests = captures.lock().unwrap();
    assert_eq!(requests.len(), 8);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.line.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn unknown_or_missing_team_role_keeps_tasks_read_only_without_a_post() {
    for role in [None, Some("FUTURE_ROLE")] {
        let disk = DiskRecovery::new();
        let (client, worker, captures) = server(5, move |request, _| {
            if request.line.starts_with("GET /api/v1/projects?") {
                Some(Response::json(
                    json!({"results":[{"id":"project","name":"Read only","color":"blue","child_order":0,"workspace_id":"team","role":role}],"next_cursor":null}),
                ))
            } else {
                Some(loaded(request))
            }
        });
        let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
        let data = planner.refresh().await.unwrap();
        assert_eq!(data.tasks.len(), 1);
        assert!(matches!(
            planner
                .submit(TaskChange::Complete(Id::parse("task").unwrap()))
                .await,
            Err(ChangeError::Transport(TransportError::Domain(
                Error::WritePermission
            )))
        ));
        assert!(!planner.can_create_in(Some(&Id::parse("project").unwrap())));
        assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
        worker.join().unwrap();
        assert!(
            captures
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.line.starts_with("GET "))
        );
    }
}

#[tokio::test]
async fn oversized_account_can_load_calendar_then_complete_project_without_partial_rows() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(117, |request, _| {
        if request.line.starts_with("GET /api/v1/tasks/filter?") {
            assert!(query(request).contains(&("query".into(), "overdue | 7 days".into())));
            let mut dated = task("dated");
            dated["due"] = json!({"date":"2026-11-02","is_recurring":false});
            return Some(Response::json(
                json!({"results":[dated],"next_cursor":null}),
            ));
        }
        if request.line.starts_with("GET /api/v1/tasks?") {
            let query = query(request);
            if query.contains(&("project_id".into(), "project".into())) {
                return Some(Response::json(
                    json!({"results":[task("undated")],"next_cursor":null}),
                ));
            }
            let page = query
                .iter()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.parse::<usize>().unwrap())
                .unwrap_or(0);
            let rows: Vec<_> = (0..if page == 50 { 1 } else { 100 })
                .map(|index| task(&format!("task-{}", page * 100 + index)))
                .collect();
            return Some(Response::json(
                json!({"results":rows,"next_cursor":if page==50 {None} else {Some((page+1).to_string())}}),
            ));
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    assert!(matches!(
        planner.refresh().await,
        Err(TransportError::Domain(Error::ReadLimit))
    ));
    assert!(planner.data().is_none());
    assert!(!planner.can_submit());
    assert_eq!(
        planner.catalog().unwrap().projects[0].id.as_str(),
        "project"
    );
    let data = planner.refresh_scope(Coverage::Calendar).await.unwrap();
    assert!(data.coverage == Coverage::Calendar);
    assert_eq!(data.tasks.len(), 1);
    assert_eq!(data.tasks[0].id.as_str(), "dated");
    assert!(planner.can_submit());
    assert!(matches!(
        planner.refresh_scope(Coverage::All).await,
        Err(TransportError::Domain(Error::ReadLimit))
    ));
    assert!(Arc::ptr_eq(planner.data().unwrap(), &data));
    assert!(planner.coverage() == &Coverage::Calendar);
    assert!(!planner.can_submit());
    let data = planner
        .refresh_scope(Coverage::Project(Id::parse("project").unwrap()))
        .await
        .unwrap();
    assert!(data.coverage == Coverage::Project(Id::parse("project").unwrap()));
    assert_eq!(data.tasks.len(), 1);
    assert_eq!(data.tasks[0].id.as_str(), "undated");
    assert!(data.tasks[0].due.is_none());
    worker.join().unwrap();
    let requests = captures.lock().unwrap();
    assert_eq!(requests.len(), 117);
    assert!(
        requests
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
}
