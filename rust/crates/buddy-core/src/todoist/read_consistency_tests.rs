use super::*;
use crate::todoist::planner::Planner;

fn form(request: &Captured) -> std::collections::HashMap<String, String> {
    Url::parse(&format!("http://127.0.0.1/?{}", request.body))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

#[tokio::test]
async fn concurrent_cursor_change_cannot_replace_the_retained_dataset_with_skipped_rows() {
    for resource in ["items", "projects"] {
        let disk = DiskRecovery::new();
        let mut syncs = 0;
        let (client, worker, captures) = server(21, move |request, index| {
            if request.line == "POST /api/v1/sync HTTP/1.1" {
                let fields = form(request);
                assert_eq!(fields.len(), 2);
                assert!(!fields.contains_key("commands"));
                syncs += 1;
                if syncs == 1 {
                    assert_eq!(fields["sync_token"], "*");
                    assert_eq!(fields["resource_types"], "[\"user\"]");
                    return Some(Response::json(
                        json!({"sync_token":"old-marker","full_sync":true,
                        "user":{"id":"planner-account","tz_info":{"timezone":"America/New_York"}}}),
                    ));
                }
                assert_eq!(fields["resource_types"], "[\"items\",\"projects\"]");
                assert_eq!(
                    fields["sync_token"],
                    match syncs {
                        2 => "old-marker",
                        3 => "read-marker",
                        _ => "changed-marker",
                    }
                );
                // A delayed full-sync token is advanced before REST page one.
                let mut reply = json!({"sync_token":if syncs==2 {"read-marker"}else{"changed-marker"},"full_sync":false,"items":[],"projects":[]});
                if syncs == 2 {
                    reply["items"] = json!([{"id":"changed-before-read"}]);
                }
                if syncs == 3 {
                    reply[resource] = json!([{"id":"skipped-middle"}]);
                }
                return Some(Response::json(reply));
            }
            if request.line.starts_with("GET /api/v1/tasks?") && index >= 7 {
                let following = query(request).iter().any(|(key, _)| key == "cursor");
                return Some(Response::json(if !following {
                    json!({"results":[task("first")],"next_cursor":"tail"})
                } else if index < 13 {
                    // No duplicate ID reveals that this cursor missed a row.
                    json!({"results":[task("last")],"next_cursor":null})
                } else {
                    json!({"results":[task("skipped-middle"),task("last")],"next_cursor":null})
                }));
            }
            Some(planner_tests::loaded(request))
        });
        let mut planner = Planner::connect(client, disk.config(), None).await.unwrap();
        let original = planner.refresh().await.unwrap();
        assert!(
            planner.refresh().await.is_err(),
            "A terminal cursor cannot prove that no row was skipped"
        );
        assert!(Arc::ptr_eq(planner.data().unwrap(), &original));
        assert!(!planner.can_submit());
        assert_eq!(
            captures.lock().unwrap().len(),
            13,
            "A failed consistency check must not retry automatically"
        );
        let current = planner.refresh().await.unwrap();
        assert_eq!(
            current
                .tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "skipped-middle", "last"]
        );
        assert!(planner.can_submit());
        worker.join().unwrap();
        let traffic = captures.lock().unwrap();
        assert_eq!(traffic.len(), 21);
        assert_eq!(
            traffic
                .iter()
                .filter(|request| request.line.starts_with("POST "))
                .count(),
            5
        );
        assert!(
            traffic
                .iter()
                .filter(|request| request.line.starts_with("POST "))
                .all(|request| {
                    let fields = form(request);
                    fields.len() == 2
                        && fields.contains_key("sync_token")
                        && fields.contains_key("resource_types")
                        && !fields.contains_key("commands")
                })
        );
        assert_eq!(std::fs::read_dir(&disk.0).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn malformed_sync_bootstrap_never_publishes_a_provisional_first_page() {
    for invalid in [
        json!({"sync_token":"","full_sync":true,"user":{"id":"account","tz_info":{}}}),
        json!({"sync_token":"*","full_sync":true,"user":{"id":"account","tz_info":{}}}),
        json!({"sync_token":"marker","full_sync":false,"user":{"id":"account","tz_info":{}}}),
        json!({"sync_token":"marker","full_sync":true}),
    ] {
        let (client, worker, captures) = server(2, move |request, index| {
            Some(if index == 0 {
                assert!(request.line.starts_with("GET /api/v1/tasks?"));
                Response::json(json!({"results":[task("provisional")],"next_cursor":"tail"}))
            } else {
                assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
                Response::json(invalid.clone())
            })
        });
        assert!(matches!(
            client.tasks(&ReadScope::Active).await,
            Err(TransportError::Domain(Error::InvalidResponse))
        ));
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn sync_errors_are_not_retried_or_replaced_by_a_partial_collection() {
    for (status, expected) in [
        (401, TransportError::Authentication),
        (429, TransportError::RateLimited),
        (503, TransportError::Service),
    ] {
        let (client, worker, captures) = server(2, move |_, index| {
            Some(if index == 0 {
                Response::json(json!({"results":[task("provisional")],"next_cursor":"tail"}))
            } else {
                Response {
                    status,
                    body: "private service body".into(),
                    headers: String::new(),
                }
            })
        });
        assert!(matches!(client.tasks(&ReadScope::Active).await, Err(error) if error == expected));
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), 2);
        assert!(!expected.to_string().contains("private service body"));
    }
}

#[tokio::test]
async fn an_expired_watermark_requires_a_new_bootstrap_on_the_next_explicit_read() {
    let (client, worker, captures) = server(12, |request, index| {
        if index == 5 {
            assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
            return Some(Response::json(
                json!({"sync_token":"stable-marker","full_sync":true,"items":[],"projects":[]}),
            ));
        }
        if index == 11 {
            // An unchanged incremental response may omit resource arrays.
            return Some(Response::json(
                json!({"sync_token":"stable-marker","full_sync":false}),
            ));
        }
        if let Some(reply) = sync_read_reply(request) {
            return Some(reply);
        }
        let next = query(request).iter().any(|(key, _)| key == "cursor");
        Some(Response::json(if next {
            json!({"results":[task("last")],"next_cursor":null})
        } else {
            json!({"results":[task("first")],"next_cursor":"tail"})
        }))
    });
    assert!(matches!(
        client.tasks(&ReadScope::Active).await,
        Err(TransportError::Domain(Error::ChangedWhileLoading))
    ));
    assert_eq!(captures.lock().unwrap().len(), 6);
    let current = client.tasks(&ReadScope::Active).await.unwrap();
    assert_eq!(
        current
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    worker.join().unwrap();
    let traffic = captures.lock().unwrap();
    assert_eq!(traffic.len(), 12);
    assert_eq!(
        traffic
            .iter()
            .filter(
                |request| request.line.starts_with("POST ") && form(request)["sync_token"] == "*"
            )
            .count(),
        2
    );
}
