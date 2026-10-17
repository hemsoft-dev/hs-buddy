use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::*;
use crate::todoist::{CreateDraft, Priority, WriteAttempt, WritePhase};

#[path = "acknowledgement_tests.rs"]
mod acknowledgement_tests;
#[path = "change_identity_tests.rs"]
mod change_identity_tests;
#[path = "planner_tests.rs"]
mod planner_tests;

struct Captured {
    line: String,
    headers: String,
    body: String,
}

struct DiskRecovery(std::path::PathBuf);
impl DiskRecovery {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("buddy-todoist-send-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> std::path::PathBuf {
        self.0.join("config.json")
    }
    fn pending(&self) -> std::path::PathBuf {
        std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|value| value == "json"))
            .unwrap()
    }
}
impl Drop for DiskRecovery {
    fn drop(&mut self) {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let path = self.0.canonicalize().unwrap();
        assert_eq!(path.parent(), Some(root.as_path()));
        assert!(
            !std::fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn persisted_dispatch_survives_unknown_response_and_restart_without_implicit_resend() {
    use crate::todoist::{WriteKind, change::SavedChange};
    let fixture = DiskRecovery::new();
    let mut original_commands = None;
    let mut unique_commands = HashSet::new();
    let (client, worker, captures) = server(6, move |request, index| {
        if matches!(index, 0 | 1 | 3 | 4) {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            return Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})));
        }
        assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
        let form: Vec<_> = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(form.len(), 1);
        let commands = &form[0].1;
        if let Some(original) = &original_commands {
            assert_eq!(original, commands);
        } else {
            original_commands = Some(commands.clone());
        }
        let value: Value = serde_json::from_str(commands).unwrap();
        let id = value[0]["uuid"].as_str().unwrap();
        unique_commands.insert(id.to_owned());
        assert_eq!(unique_commands.len(), 1);
        if index == 2 {
            None
        } else {
            Some(Response::json(json!({"sync_status":{id:"ok"}})))
        }
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let original_id = request.command_id();
    let original_payload = request.commands().to_owned();
    let mut change = SavedChange::capture(&client, &fixture.config(), request)
        .await
        .unwrap();
    let stored = std::fs::read(fixture.pending()).unwrap();
    assert_eq!(change.request().command_id(), original_id);
    assert_eq!(change.request().commands(), original_payload);
    assert_eq!(change.phase(), WritePhase::Ready);
    assert_eq!(
        captures.lock().unwrap().len(),
        1,
        "capture must not dispatch"
    );
    let report = change.dispatch(&client).await.unwrap();
    assert_eq!(report.phase, WritePhase::Unconfirmed);
    assert_eq!(report.transport_problem, Some(TransportError::Unavailable));
    assert!(report.cleanup_problem.is_none());
    assert_eq!(
        captures.lock().unwrap().len(),
        3,
        "lost response must not retry"
    );
    assert_eq!(std::fs::read(fixture.pending()).unwrap(), stored);
    drop(change);
    let reconnected = Client::build("synthetic-token", client.base.clone(), false).unwrap();
    let mut restored = SavedChange::recover(&reconnected, &fixture.config())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.phase(), WritePhase::Unconfirmed);
    assert_eq!(restored.request().kind(), WriteKind::Complete);
    assert_eq!(restored.request().command_id(), original_id);
    assert_eq!(restored.request().commands(), original_payload);
    assert_eq!(restored.phase(), WritePhase::Unconfirmed);
    assert_eq!(std::fs::read(fixture.pending()).unwrap(), stored);
    assert_eq!(
        captures.lock().unwrap().len(),
        4,
        "recovery must only read identity"
    );
    let report = restored.dispatch(&reconnected).await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert!(report.transport_problem.is_none() && report.cleanup_problem.is_none());
    assert_eq!(report.task.unwrap().as_str(), "task");
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn changed_authenticated_identity_blocks_dispatch_and_keeps_original_recovery() {
    use crate::todoist::change::{ChangeError, SavedChange};
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(2, |request, index| {
        assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
        Some(Response::json(
            json!({"id":if index == 0 { "original-user" } else { "foreign-user" }, "tz_info":{}}),
        ))
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let mut change = SavedChange::capture(&client, &fixture.config(), request)
        .await
        .unwrap();
    let stored = std::fs::read(fixture.pending()).unwrap();
    assert!(matches!(
        change.dispatch(&client).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WrongConnection
        )))
    ));
    assert_eq!(change.phase(), WritePhase::Ready);
    assert_eq!(std::fs::read(fixture.pending()).unwrap(), stored);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn definite_acknowledgement_survives_changed_recovery_and_cleanup_does_not_resend() {
    use crate::todoist::change::{ChangeError, SavedChange};
    use crate::todoist::storage::StorageError;
    let fixture = DiskRecovery::new();
    let directory = fixture.0.clone();
    let (client, worker, captures) = server(3, move |request, index| {
        if index < 2 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            return Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})));
        }
        assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
        let form: Vec<_> = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        let commands: Value = serde_json::from_str(&form[0].1).unwrap();
        let id = commands[0]["uuid"].as_str().unwrap();
        let path = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(path, b"external recovery must be retained").unwrap();
        Some(Response::json(json!({"sync_status":{id:"ok"}})))
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let mut change = SavedChange::capture(&client, &fixture.config(), request)
        .await
        .unwrap();
    let report = change.dispatch(&client).await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert_eq!(report.cleanup_problem, Some(StorageError::Changed));
    assert!(report.transport_problem.is_none());
    assert_eq!(
        change.retry_cleanup(),
        Err(ChangeError::Storage(StorageError::Changed))
    );
    assert!(matches!(
        change.dispatch(&client).await,
        Err(ChangeError::Transport(TransportError::Domain(
            Error::WriteBlocked
        )))
    ));
    assert_eq!(
        std::fs::read(fixture.pending()).unwrap(),
        b"external recovery must be retained"
    );
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn a_busy_pending_file_blocks_send_before_any_post() {
    use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};
    use crate::todoist::change::{ChangeError, SavedChange};
    use crate::todoist::storage::StorageError;
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(2, |request, _| {
        assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
        Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})))
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let mut change = SavedChange::capture(&client, &fixture.config(), request)
        .await
        .unwrap();
    let stored = std::fs::read(fixture.pending()).unwrap();
    let lock = ConfigLock::acquire(&fixture.pending(), DEFAULT_TIMEOUT)
        .unwrap()
        .unwrap();
    assert!(matches!(
        change.dispatch(&client).await,
        Err(ChangeError::Storage(StorageError::Busy))
    ));
    assert_eq!(change.phase(), WritePhase::Ready);
    assert_eq!(std::fs::read(fixture.pending()).unwrap(), stored);
    drop(lock);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

fn recovery_path() -> &'static std::path::Path {
    std::path::Path::new(if cfg!(windows) {
        "D:/synthetic-unused/recovery-profile"
    } else {
        "/synthetic-unused/recovery-profile"
    })
}

#[tokio::test]
async fn reopening_sends_one_persisted_uncomplete_command_and_retires_only_after_ack() {
    use crate::todoist::change::SavedChange;
    let fixture = DiskRecovery::new();
    let (client, worker, captures) = server(3, |request, index| {
        if index < 2 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            return Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})));
        }
        assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
        let form: Vec<_> = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(form.len(), 1);
        let value: Value = serde_json::from_str(&form[0].1).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["type"], "item_uncomplete");
        assert_eq!(value[0]["args"], json!({"id":"completed-task"}));
        let id = value[0]["uuid"].as_str().unwrap();
        Some(Response::json(json!({"sync_status":{id:"ok"}})))
    });
    let request =
        WriteRequest::reopen(client.write_scope(), &Id::parse("completed-task").unwrap()).unwrap();
    let original = request.commands().to_owned();
    let mut change = SavedChange::capture(&client, &fixture.config(), request)
        .await
        .unwrap();
    let stored: Value = serde_json::from_slice(&std::fs::read(fixture.pending()).unwrap()).unwrap();
    assert_eq!(stored["commands"], original);
    assert_eq!(
        captures.lock().unwrap().len(),
        1,
        "capture must persist before dispatch"
    );
    let report = change.dispatch(&client).await.unwrap();
    assert_eq!(report.phase, WritePhase::Confirmed);
    assert_eq!(report.task.unwrap().as_str(), "completed-task");
    assert!(report.transport_problem.is_none() && report.cleanup_problem.is_none());
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn restart_verifies_account_and_only_explicit_retry_sends_the_original_command() {
    let (original, worker, captures) = server(1, |request, _| {
        assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
        Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})))
    });
    let request =
        WriteRequest::complete(original.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let commands = request.commands().to_owned();
    let command_id = request.command_id();
    let attempt = WriteAttempt::new(request);
    let bytes = original
        .checkpoint(&attempt, recovery_path())
        .await
        .unwrap();
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 1);
    let expected = commands.clone();
    let (reconnected, worker, captures) = server(2, move |request, index| {
        if index == 0 {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            Some(Response::json(json!({"id":"synthetic-user", "tz_info":{}})))
        } else {
            assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
            let form: Vec<_> = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            assert_eq!(form, vec![("commands".into(), expected.clone())]);
            Some(Response::json(
                json!({"sync_status":{command_id.to_string():"ok"}}),
            ))
        }
    });
    let mut recovered = reconnected.recover(&bytes, recovery_path()).await.unwrap();
    assert_eq!(recovered.phase(), WritePhase::Unconfirmed);
    assert_eq!(
        captures.lock().unwrap().len(),
        1,
        "recovery must only read identity"
    );
    let request = recovered.begin(reconnected.write_scope()).unwrap();
    assert_eq!(request.commands(), commands);
    let reply = reconnected.send(&request).await.unwrap();
    assert!(recovered.observe(request.command_id(), reply.outcome));
    assert_eq!(recovered.phase(), WritePhase::Confirmed);
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_foreign_or_failed_account_read_cannot_recover_or_send_a_saved_change() {
    let (original, worker, _) = server(1, |_, _| {
        Some(Response::json(json!({"id":"original-user", "tz_info":{}})))
    });
    let attempt = WriteAttempt::new(
        WriteRequest::delete(original.write_scope(), &Id::parse("task").unwrap()).unwrap(),
    );
    let bytes = original
        .checkpoint(&attempt, recovery_path())
        .await
        .unwrap();
    worker.join().unwrap();
    for status in [200, 401] {
        let (other, worker, captures) = server(1, move |request, _| {
            assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
            Some(Response {
                status,
                body: json!({"id":"foreign-user", "tz_info":{}}).to_string(),
                headers: String::new(),
            })
        });
        let result = other.recover(&bytes, recovery_path()).await;
        assert!(matches!(
            (status, result),
            (200, Err(TransportError::Domain(Error::WrongConnection)))
                | (401, Err(TransportError::Authentication))
        ));
        worker.join().unwrap();
        assert_eq!(captures.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn profile_reads_the_captured_accounts_zone_and_preserves_an_absent_zone() {
    let (client, worker, captures) = server(2, |request, index| {
        assert_eq!(request.line, "GET /api/v1/user HTTP/1.1");
        Some(Response::json(if index == 0 {
            json!({"id":"synthetic-user", "tz_info":{"timezone":"Pacific/Auckland"}})
        } else {
            json!({"id":"synthetic-user", "tz_info":{}})
        }))
    });
    let first = client.profile().await.unwrap();
    assert!(
        first.id.as_str() == "synthetic-user"
            && first.timezone.as_deref() == Some("Pacific/Auckland")
    );
    assert!(client.profile().await.unwrap().timezone.is_none());
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn profile_authentication_failure_does_not_return_an_earlier_identity() {
    let (client, worker, captures) = server(2, |_, index| {
        Some(if index == 0 {
            Response::json(json!({"id":"synthetic-user", "tz_info":{"timezone":"Europe/Madrid"}}))
        } else {
            Response {
                status: 401,
                body: "synthetic-private-error".into(),
                headers: String::new(),
            }
        })
    });
    assert!(client.profile().await.is_ok());
    assert!(matches!(
        client.profile().await,
        Err(TransportError::Authentication)
    ));
    worker.join().unwrap();
    assert_eq!(captures.lock().unwrap().len(), 2);
}

#[test]
fn missing_deleted_or_invalid_account_context_never_acquires_a_machine_zone() {
    use crate::todoist::parse_profile;
    for value in [
        json!({"id":"user"}),
        json!({"id":"", "tz_info":{}}),
        json!({"id":"user", "is_deleted":true, "tz_info":{"timezone":"UTC"}}),
        json!({"id":"user", "tz_info":{"timezone":""}}),
        json!({"id":"user", "tz_info":{"timezone":"invalid\nzone"}}),
    ] {
        assert!(parse_profile(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(matches!(
        parse_profile(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Err(Error::ResponseTooLarge)
    ));
}
struct Response {
    status: u16,
    body: String,
    headers: String,
}
impl Response {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: value.to_string(),
            headers: String::new(),
        }
    }
}
fn capture(stream: &mut TcpStream) -> Captured {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 32 * 1024, "test headers exceed limit");
        if bytes.ends_with(b"\r\n\r\n") {
            break bytes.len();
        }
    };
    let headers = String::from_utf8(bytes.clone()).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 2 * MAX_RESPONSE_BYTES);
    bytes.resize(end + length, 0);
    stream.read_exact(&mut bytes[end..]).unwrap();
    Captured {
        line: headers.lines().next().unwrap().into(),
        headers,
        body: String::from_utf8(bytes[end..].to_vec()).unwrap(),
    }
}
fn server(
    count: usize,
    mut handler: impl FnMut(&Captured, usize) -> Option<Response> + Send + 'static,
) -> (Client, thread::JoinHandle<()>, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let client = Client::build(
        "synthetic-todoist-token",
        Url::parse(&format!("http://{address}/api/v1/")).unwrap(),
        false,
    )
    .unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let recorded = captures.clone();
    let worker = thread::spawn(move || {
        for index in 0..count {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => panic!("owned test server did not receive expected request"),
                }
            };
            let request = capture(&mut stream);
            if let Some(reply) = handler(&request, index) {
                let response = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.headers,
                    reply.body
                );
                if stream.write_all(response.as_bytes()).is_err() {
                    assert!(
                        reply.body.len() > MAX_RESPONSE_BYTES,
                        "unexpected early response close"
                    );
                }
            }
            recorded.lock().unwrap().push(request);
        }
    });
    (client, worker, captures)
}
fn task(id: &str) -> Value {
    json!({"id":id,"content":"Synthetic task","description":"","project_id":"project", "priority":1,"due":null,"labels":[],"checked":false,"child_order":0})
}
fn query(request: &Captured) -> Vec<(String, String)> {
    let path = request.line.split_whitespace().nth(1).unwrap();
    Url::parse(&format!("http://127.0.0.1{path}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

#[tokio::test]
async fn real_http_pagination_keeps_empty_intermediate_pages_and_encodes_private_filters() {
    let (client, worker, captures) = server(3, |request, index| {
        let values = query(request);
        assert!(request.line.starts_with("GET /api/v1/tasks/filter?"));
        assert!(values.contains(&("query".into(), "today & #Synthetic Unicode π".into())));
        assert!(values.contains(&("limit".into(), "100".into())));
        let cursor = values
            .iter()
            .find(|(key, _)| key == "cursor")
            .map(|(_, value)| value.as_str());
        assert_eq!(
            cursor,
            [None, Some("opaque &one"), Some("opaque/two")][index]
        );
        Some(Response::json(match index {
            0 => json!({"results":[task("first")],"next_cursor":"opaque &one"}),
            1 => json!({"results":[],"next_cursor":"opaque/two"}),
            _ => json!({"results":[task("last")],"next_cursor":null}),
        }))
    });
    let tasks = client
        .tasks(&ReadScope::Filter("today & #Synthetic Unicode π".into()))
        .await
        .unwrap();
    worker.join().unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    assert_eq!(captures.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn a_later_http_failure_returns_no_partial_snapshot_or_private_service_error() {
    let (client, worker, _) = server(2, |_, index| {
        Some(if index == 0 {
            Response::json(json!({"results":[task("first")],"next_cursor":"next"}))
        } else {
            Response {
                status: 503,
                body: "private service diagnostic".into(),
                headers: String::new(),
            }
        })
    });
    let result = client.tasks(&ReadScope::Active).await;
    worker.join().unwrap();
    assert!(matches!(result, Err(TransportError::Service)));
    assert!(
        !TransportError::Service
            .to_string()
            .contains("private service diagnostic")
    );
}

#[tokio::test]
async fn lost_commit_response_requires_explicit_retry_of_same_form_uuid_and_creates_once() {
    let mut seen = HashSet::new();
    let (client, worker, captures) = server(2, move |request, index| {
        assert_eq!(request.line, "POST /api/v1/sync HTTP/1.1");
        assert!(
            request
                .headers
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        let params: Vec<_> = Url::parse(&format!("http://127.0.0.1/?{}", request.body))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "commands");
        let commands: Value = serde_json::from_str(&params[0].1).unwrap();
        let command = &commands[0];
        assert_eq!(command["type"], "item_add");
        let uuid = command["uuid"].as_str().unwrap().to_owned();
        let temp = command["temp_id"].as_str().unwrap();
        let is_new = seen.insert(uuid.clone());
        assert_eq!(
            is_new,
            index == 0,
            "explicit retry must reuse original command UUID"
        );
        assert_eq!(seen.len(), 1, "service applies one unique command");
        if index == 0 {
            None
        } else {
            Some(Response::json(
                json!({"sync_status":{uuid:"ok"},"temp_id_mapping":{temp:"created-task"}}),
            ))
        }
    });
    let draft = CreateDraft {
        content: "Synthetic & Unicode π".into(),
        description: String::new(),
        project: None,
        priority: Priority::from_api(4).unwrap(),
        due: None,
    };
    let mut attempt =
        WriteAttempt::new(WriteRequest::create(client.write_scope(), &draft).unwrap());
    let first = attempt.begin(client.write_scope()).unwrap();
    let reply = client.send(&first).await.unwrap();
    assert!(matches!(reply.outcome, WriteOutcome::Unconfirmed));
    assert_eq!(reply.problem, Some(TransportError::Unavailable));
    assert!(attempt.observe(first.command_id(), reply.outcome));
    assert_eq!(attempt.phase(), WritePhase::Unconfirmed);
    assert_eq!(
        captures.lock().unwrap().len(),
        1,
        "transport must not retry implicitly"
    );
    let retry = attempt.begin(client.write_scope()).unwrap();
    assert!(Arc::ptr_eq(&first, &retry));
    let reply = client.send(&retry).await.unwrap();
    assert!(matches!(&reply.outcome, WriteOutcome::Confirmed(id) if id.as_str() == "created-task"));
    attempt.observe(retry.command_id(), reply.outcome);
    assert_eq!(attempt.phase(), WritePhase::Confirmed);
    worker.join().unwrap();
    let requests = captures.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].body == requests[1].body,
        "retry payload changed"
    );
}

#[tokio::test]
async fn a_different_connection_is_rejected_before_any_http() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = Client::build(
        "synthetic-todoist-token",
        Url::parse(&format!(
            "http://{}/api/v1/",
            listener.local_addr().unwrap()
        ))
        .unwrap(),
        false,
    )
    .unwrap();
    let request = WriteRequest::complete(WriteScope::new(), &Id::parse("task").unwrap()).unwrap();
    assert!(matches!(
        client.send(&request).await,
        Err(TransportError::Domain(Error::WrongConnection))
    ));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn redirect_is_not_followed_or_forwarded_and_write_remains_unconfirmed() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let location = format!(
        "Location: http://{}/credential-sink\r\n",
        target.local_addr().unwrap()
    );
    let (client, worker, _) = server(1, move |_, _| {
        Some(Response {
            status: 307,
            body: String::new(),
            headers: location.clone(),
        })
    });
    let request =
        WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
    let reply = client.send(&request).await.unwrap();
    worker.join().unwrap();
    assert!(matches!(reply.outcome, WriteOutcome::Unconfirmed));
    assert_eq!(reply.problem, Some(TransportError::Redirect));
    assert!(
        matches!(target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn oversized_success_response_does_not_publish_partial_tasks() {
    let (client, worker, _) = server(1, |_, _| {
        Some(Response {
            status: 200,
            body: " ".repeat(MAX_RESPONSE_BYTES + 1),
            headers: String::new(),
        })
    });
    let result = client.tasks(&ReadScope::Active).await;
    worker.join().unwrap();
    assert!(matches!(
        result,
        Err(TransportError::Domain(Error::ResponseTooLarge))
    ));
}

#[test]
fn production_origin_and_sensitive_header_are_fixed_and_invalid_credentials_stay_private() {
    let client = Client::new("synthetic-todoist-token").unwrap();
    assert_eq!(client.base.as_str(), API);
    assert!(client.authorization.is_sensitive());
    for invalid in ["", "synthetic token", "synthetic\r\nInjected: yes"] {
        assert!(matches!(
            Client::new(invalid),
            Err(TransportError::InvalidCredential)
        ));
    }
}

#[tokio::test]
async fn authentication_rate_limit_and_malformed_acknowledgment_do_not_confirm_or_replay_writes() {
    for (status, expected) in [
        (401, TransportError::Authentication),
        (429, TransportError::RateLimited),
        (200, TransportError::Domain(Error::InvalidResponse)),
    ] {
        let (client, worker, captures) = server(1, move |_, _| {
            Some(Response {
                status,
                body: "private unsupported service body".into(),
                headers: String::new(),
            })
        });
        let request =
            WriteRequest::complete(client.write_scope(), &Id::parse("task").unwrap()).unwrap();
        let reply = client.send(&request).await.unwrap();
        worker.join().unwrap();
        assert!(matches!(reply.outcome, WriteOutcome::Unconfirmed));
        assert_eq!(reply.problem, Some(expected));
        assert_eq!(captures.lock().unwrap().len(), 1);
        assert!(
            !expected
                .to_string()
                .contains("private unsupported service body")
        );
    }
}
