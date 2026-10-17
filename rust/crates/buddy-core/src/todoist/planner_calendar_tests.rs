use super::*;
use crate::todoist::planner::Coverage;

#[tokio::test]
async fn same_account_replacement_loads_calendar_after_its_retained_project_disappears() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(12, |request, index| {
        if index >= 5 {
            assert!(request.headers.contains("Bearer replacement-token"));
        }
        if index >= 5 && request.line.starts_with("GET /api/v1/projects?") {
            return Some(Response::json(
                json!({"results":[{"id":"new-project","name":"Available project","color":"blue","child_order":0}],"next_cursor":null}),
            ));
        }
        if request.line.starts_with("GET /api/v1/tasks/filter?") {
            assert!(query(request).contains(&("query".into(), "overdue | 7 days".into())));
            let mut row = task("fallback-task");
            row["project_id"] = json!("new-project");
            row["due"] = json!({"date":"2026-11-03"});
            return Some(Response::json(json!({"results":[row],"next_cursor":null})));
        }
        Some(loaded(request))
    });
    let base = client.base.clone();
    let mut original = Planner::connect(client, disk.config(), None).await.unwrap();
    let previous = original
        .refresh_scope(Coverage::Project(Id::parse("project").unwrap()))
        .await
        .unwrap();
    let replacement = Client::build("replacement-token", base, false).unwrap();
    let mut candidate = Planner::connect(replacement, disk.config(), None)
        .await
        .unwrap();
    let data = candidate
        .refresh_connection_scope(Coverage::Project(Id::parse("project").unwrap()))
        .await
        .unwrap();
    assert!(data.coverage == Coverage::Calendar);
    assert_eq!(data.tasks[0].id.as_str(), "fallback-task");
    assert_eq!(data.account.as_str(), previous.account.as_str());
    assert!(candidate.can_submit());
    assert!(candidate.can_change_task(&data.tasks[0].id, crate::todoist::WriteKind::Complete));
    assert!(Arc::ptr_eq(original.data().unwrap(), &previous));
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 12);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
}

#[tokio::test]
async fn account_midnight_refreshes_the_calendar_window_once_without_sending_a_change() {
    let disk = DiskRecovery::new();
    let before = "2026-11-02T23:59:00Z".parse().unwrap();
    let same_day = "2026-11-03T00:01:00Z".parse().unwrap();
    let after = "2026-11-03T05:00:01Z".parse().unwrap();
    let (client, worker, captures) = server(9, |request, index| {
        if request.line.starts_with("GET /api/v1/tasks/filter?") {
            assert!(query(request).contains(&("query".into(), "overdue | 7 days".into())));
            let mut today = task("new-today");
            today["due"] = json!({"date":"2026-11-03"});
            let mut end = task(if index < 5 {
                "old-seventh-day"
            } else {
                "new-seventh-day"
            });
            end["due"] = json!({"date":if index<5 {"2026-11-08"}else{"2026-11-09"}});
            return Some(Response::json(
                json!({"results":[today,end],"next_cursor":null}),
            ));
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), Some(before))
        .await
        .unwrap();
    let old = planner.refresh_scope(Coverage::Calendar).await.unwrap();
    assert!(
        old.calendar.schedule(&old.tasks, old.now(), 1).unwrap()[0]
            .tasks
            .is_empty()
    );
    assert!(
        !old.needs_day_refresh(same_day).unwrap(),
        "UTC midnight is not this account's midnight"
    );
    assert!(planner.refresh_day_at(same_day).await.unwrap().is_none());
    assert!(Arc::ptr_eq(planner.data().unwrap(), &old));
    let new = planner.refresh_day_at(after).await.unwrap().unwrap();
    assert_eq!(new.read_at, after);
    assert_eq!(new.now(), after);
    assert!(new.coverage == Coverage::Calendar);
    assert_eq!(new.tasks[1].id.as_str(), "new-seventh-day");
    let today = new.calendar.schedule(&new.tasks, new.now(), 1).unwrap();
    assert_eq!(today.len(), 1);
    assert_eq!(today[0].tasks[0].id.as_str(), "new-today");
    assert!(planner.refresh_day_at(after).await.unwrap().is_none());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 9);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
}

#[tokio::test]
async fn unavailable_project_can_fall_back_to_a_project_when_calendar_also_exceeds_the_bound() {
    let disk = DiskRecovery::new();
    let (client, worker, captures) = server(64, |request, _| {
        if let Some(reply) = sync_read_reply(request) {
            return Some(reply);
        }
        if request.line.starts_with("GET /api/v1/projects?") {
            let mut projects = vec![
                json!({"id":"project","name":"Available Inbox","color":"blue","child_order":0}),
            ];
            projects.extend((0..17).map(|index|json!({"id":format!("bulk-project-{index}"),"name":format!("Synthetic {index}"),"color":"blue","child_order":index+1})));
            return Some(Response::json(
                json!({"results":projects,"next_cursor":null}),
            ));
        }
        if request.line.starts_with("GET /api/v1/tasks/filter?") {
            let query = query(request);
            let page = query
                .iter()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.parse::<usize>().unwrap())
                .unwrap_or(0);
            let rows: Vec<_> = (0..if page == 50 { 1 } else { 100 })
                .map(|offset| {
                    let index = page * 100 + offset;
                    let mut row = task(&format!("bulk-{index}"));
                    row["project_id"] = json!(format!("bulk-project-{}", index / 300));
                    row["due"] = json!({"date":"2026-11-03"});
                    row
                })
                .collect();
            return Some(Response::json(
                json!({"results":rows,"next_cursor":if page==50 {None}else{Some((page+1).to_string())}}),
            ));
        }
        if request.line.starts_with("GET /api/v1/tasks?") {
            assert!(query(request).contains(&("project_id".into(), "project".into())));
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
    let data = planner
        .refresh_connection_scope(Coverage::Project(Id::parse("removed").unwrap()))
        .await
        .unwrap();
    assert!(data.coverage == Coverage::Project(Id::parse("project").unwrap()));
    assert_eq!(data.tasks.len(), 1);
    assert!(data.tasks[0].due.is_none());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 64);
    assert!(captures.lock().unwrap().iter().all(read_only_request));
}

#[tokio::test]
async fn failed_day_refresh_retains_the_completed_scope_and_disables_stale_writes() {
    let disk = DiskRecovery::new();
    let before = "2026-11-02T23:59:00Z".parse().unwrap();
    let (client, worker, captures) = server(8, |request, index| {
        if index == 7 {
            assert!(request.line.starts_with("GET /api/v1/tasks/filter?"));
            return Some(Response {
                status: 503,
                body: String::new(),
                headers: String::new(),
            });
        }
        if request.line.starts_with("GET /api/v1/tasks/filter?") {
            return Some(Response::json(
                json!({"results":[task("retained")],"next_cursor":null}),
            ));
        }
        Some(loaded(request))
    });
    let mut planner = Planner::connect(client, disk.config(), Some(before))
        .await
        .unwrap();
    let original = planner.refresh_scope(Coverage::Calendar).await.unwrap();
    assert!(matches!(
        planner
            .refresh_day_at("2026-11-03T05:00:01Z".parse().unwrap())
            .await,
        Err(TransportError::Service)
    ));
    assert!(Arc::ptr_eq(planner.data().unwrap(), &original));
    assert_eq!(original.read_at, before);
    assert!(!planner.can_submit());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 8);
    assert!(
        captures
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.line.starts_with("GET "))
    );
    assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
}

#[tokio::test]
async fn broad_reconnect_scopes_fall_back_to_an_available_project_after_they_grow_too_large() {
    for (preferred, expected) in [(Coverage::All, 118), (Coverage::Calendar, 62)] {
        let disk = DiskRecovery::new();
        let (client, worker, captures) = server(expected, |request, _| {
            if let Some(reply) = sync_read_reply(request) {
                return Some(reply);
            }
            if request.line.starts_with("GET /api/v1/tasks/filter?")
                || request.line.starts_with("GET /api/v1/tasks?")
                    && !query(request).iter().any(|(key, _)| key == "project_id")
            {
                let parameters = query(request);
                let page = parameters
                    .iter()
                    .find(|(key, _)| key == "cursor")
                    .map(|(_, value)| value.parse::<usize>().unwrap())
                    .unwrap_or(0);
                let rows: Vec<_> = (0..if page == 50 { 1 } else { 100 })
                    .map(|offset| {
                        let mut row = task(&format!("growing-{}", page * 100 + offset));
                        row["due"] = json!({"date":"2026-11-03"});
                        row
                    })
                    .collect();
                return Some(Response::json(
                    json!({"results":rows,"next_cursor":if page==50 {None}else{Some((page+1).to_string())}}),
                ));
            }
            Some(loaded(request))
        });
        let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
        let data = planner.refresh_connection_scope(preferred).await.unwrap();
        assert!(data.coverage == Coverage::Project(Id::parse("project").unwrap()));
        assert_eq!(data.tasks.len(), 1);
        assert!(data.tasks[0].due.is_none());
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), expected);
        assert!(captures.lock().unwrap().iter().all(read_only_request));
        assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    }
}
