use serde_json::{Value, json};

use super::*;
use crate::tempo::jira::tests::{Fixture as JiraFixture, reply as jira_reply};
use crate::tempo::transport::tests::{Fixture as TempoFixture, reply, single_worklog};
use crate::tempo::{AccountId, IssueId, parse_worklog};

#[tokio::test]
async fn review_regression_delete_expansion_preserves_displayed_baseline() {
    for changed_description in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let mut displayed = single_worklog();
        displayed["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("values");
        let baseline = parse_worklog(&serde_json::to_vec(&displayed).unwrap()).unwrap();
        assert!(baseline.attributes.is_none());
        let mut detail = displayed;
        detail["attributes"]["values"] = json!([{"key":"_Expanded_","value":"retained"}]);
        if changed_description {
            detail["description"] = json!("Changed after the list was displayed");
        }
        let mut replies = vec![reply(200, &detail.to_string())];
        if !changed_description {
            replies.push(reply(204, ""));
        }
        let identity = jira(2);
        let service = TempoFixture::new(replies);
        let mut change = SavedChange::capture(
            Arc::new(service.client()),
            Arc::new(identity.client()),
            &config,
            Draft::delete(&baseline).unwrap(),
        )
        .await
        .unwrap();
        let checkpoint = stored(&config).current().unwrap().to_vec();
        let outcome = change.dispatch().await;
        if changed_description {
            assert!(matches!(
                outcome,
                Err(ChangeError::Domain(Error::ChangedWhileLoading))
            ));
            assert_eq!(stored(&config).current(), Some(checkpoint.as_slice()));
        } else {
            assert!(outcome.unwrap().phase == WritePhase::Confirmed);
            assert!(stored(&config).current().is_none());
        }
        let requests = service.requests.lock().unwrap();
        assert_eq!(requests.len(), if changed_description { 1 } else { 2 });
        assert!(requests[0].starts_with("GET /4/worklogs/7 HTTP/1.1\r\n"));
        if !changed_description {
            assert!(requests[1].starts_with("DELETE /4/worklogs/7 HTTP/1.1\r\n"));
            assert_eq!(requests[1].split_once("\r\n\r\n").unwrap().1, "");
        }
    }
}

fn jira(count: usize) -> JiraFixture {
    JiraFixture::new(
        (0..count)
            .map(|_| {
                jira_reply(
                    200,
                    r#"{"accountId":"123456:synthetic-user","timeZone":"America/Winnipeg"}"#,
                )
            })
            .collect(),
    )
}
fn draft() -> Draft {
    let mut draft = Draft::create(
        AccountId::parse("123456:synthetic-user").unwrap(),
        IssueId::parse(19).unwrap(),
        "2026-10-16".parse().unwrap(),
        3600,
    )
    .unwrap();
    draft
        .set_description(Some("private synthetic work".to_owned()))
        .unwrap();
    draft.set_billing_account(Some("SYNTHETIC")).unwrap();
    draft
}
fn written_row() -> Value {
    let mut row = single_worklog();
    row["timeSpentSeconds"] = json!(3600);
    row["description"] = json!("private synthetic work");
    row["attributes"]["values"] = json!([{"key":"_Account_","value":"SYNTHETIC"}]);
    row
}
fn stored(config: &Path) -> PendingFile {
    PendingFile::open(config, RecoverySlot::Tempo, recovery::MAX_STORED_BYTES).unwrap()
}

/// Change only this test's owned recovery path. Restore exact permissions on unwind.
struct WriteProtection {
    #[cfg(unix)]
    file: std::path::PathBuf,
    #[cfg(unix)]
    file_permissions: std::fs::Permissions,
    #[cfg(unix)]
    parent_permissions: std::fs::Permissions,
    #[cfg(windows)]
    _read_handle: std::fs::File,
}
impl WriteProtection {
    fn apply(file: &Path) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            Self {
                // Permit reads, deny concurrent write and delete access.
                _read_handle: std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(file)
                    .unwrap(),
            }
        }
        #[cfg(unix)]
        {
            let file_permissions = std::fs::metadata(file).unwrap().permissions();
            #[cfg(unix)]
            let parent_permissions = std::fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions();
            let guard = Self {
                file: file.to_owned(),
                file_permissions,
                #[cfg(unix)]
                parent_permissions,
            };
            let mut protected = guard.file_permissions.clone();
            protected.set_readonly(true);
            std::fs::set_permissions(file, protected).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    file.parent().unwrap(),
                    std::fs::Permissions::from_mode(guard.parent_permissions.mode() & !0o222),
                )
                .unwrap();
            }
            guard
        }
    }
}
impl Drop for WriteProtection {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            std::fs::set_permissions(self.file.parent().unwrap(), self.parent_permissions.clone())
                .unwrap();
            match std::fs::set_permissions(&self.file, self.file_permissions.clone()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("Owned test permissions could not be restored: {error}"),
            }
        }
    }
}

#[tokio::test]
async fn confirmed_reply_survives_acknowledgement_and_retirement_io_failures_without_resending() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let path = stored(&config).path().to_owned();
    let protection = Arc::new(std::sync::Mutex::new(None));
    let protection_on_reply = protection.clone();
    let reply_path = path.clone();
    let service =
        TempoFixture::with_before_reply(vec![reply(200, &written_row().to_string())], move |_| {
            *protection_on_reply.lock().unwrap() = Some(WriteProtection::apply(&reply_path));
        });
    let identity = jira(2);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        draft(),
    )
    .await
    .unwrap();
    let checkpoint = std::fs::read(&path).unwrap();
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Confirmed);
    assert_eq!(report.worklog.unwrap().get(), 7);
    assert!(report.transport_problem.is_none());
    assert!(report.cleanup_problem.is_some());
    assert!(!report.acknowledgement_durable);
    assert_eq!(std::fs::read(&path).unwrap(), checkpoint);
    assert!(matches!(
        change.dispatch().await,
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    drop(protection.lock().unwrap().take());
    #[cfg(unix)]
    {
        // A read-only parent also prevents the released lease directory's removal.
        // Advance its age so cleanup exercises the normal stale-lock protocol.
        let lock_path = crate::config_lock::lock_dir(&path);
        assert_eq!(std::fs::read_dir(&lock_path).unwrap().count(), 0);
        let stale =
            std::time::SystemTime::now() - crate::config_lock::STALE_AFTER - Duration::from_secs(1);
        std::fs::File::open(&lock_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(stale))
            .unwrap();
    }
    change.retry_cleanup().unwrap();
    assert!(change.acknowledgement_durable());
    assert!(stored(&config).current().is_none());
    assert_eq!(service.requests.lock().unwrap().len(), 1);
    assert_eq!(identity.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn another_window_cannot_retire_pending_recovery_while_the_request_is_in_flight() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let peer = Arc::new(std::sync::Mutex::new(None::<PendingFile>));
    let peer_on_reply = peer.clone();
    let service =
        TempoFixture::with_before_reply(vec![reply(200, &written_row().to_string())], move |_| {
            let mut guard = peer_on_reply.lock().unwrap();
            let file = guard.as_mut().unwrap();
            assert_eq!(file.retire(), Err(StorageError::Busy));
            assert!(file.current().is_some());
            assert_eq!(
                file.while_empty(|| panic!("credentials cannot change during dispatch")),
                Err(StorageError::Busy)
            );
        });
    let identity = jira(2);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        draft(),
    )
    .await
    .unwrap();
    *peer.lock().unwrap() = Some(stored(&config));
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Confirmed && report.acknowledgement_durable);
    assert!(report.cleanup_problem.is_none());
    assert!(stored(&config).current().is_none());
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_dispatch_keeps_recovery_for_inspection_and_never_resends() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let reply_gate = gate.clone();
    let service =
        TempoFixture::with_before_reply(vec![reply(200, &written_row().to_string())], move |_| {
            let (lock, wake) = &*reply_gate;
            drop(
                wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |released| {
                    !*released
                })
                .unwrap(),
            );
        });
    let identity = jira(3);
    let jira_client = Arc::new(identity.client());
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        jira_client.clone(),
        &config,
        draft(),
    )
    .await
    .unwrap();
    let original = stored(&config).current().unwrap().to_vec();
    let task = tokio::spawn(async move { change.dispatch().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.is_err());
    let (lock, wake) = &*gate;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    assert_eq!(stored(&config).current(), Some(original.as_slice()));
    let mut recovered = SavedRecovery::open(&config, &jira_client)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered.phase() == WritePhase::Unconfirmed);
    assert!(matches!(
        recovered.cleanup(),
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    recovered.discard().unwrap();
    assert!(stored(&config).current().is_none());
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn create_is_saved_before_dispatch_and_sends_the_captured_body_exactly_once() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let identity = jira(2);
    let service = TempoFixture::new(vec![reply(200, &written_row().to_string())]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        draft(),
    )
    .await
    .unwrap();
    let checkpoint = stored(&config).current().unwrap().to_vec();
    let disk: Value = serde_json::from_slice(&checkpoint).unwrap();
    assert_eq!(disk["account"], "123456:synthetic-user");
    assert_eq!(disk["kind"], "create");
    assert!(service.requests.lock().unwrap().is_empty());
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Confirmed && report.acknowledgement_durable);
    assert_eq!(report.worklog.unwrap().get(), 7);
    assert!(report.transport_problem.is_none() && report.cleanup_problem.is_none());
    assert!(stored(&config).current().is_none());
    assert!(matches!(
        change.dispatch().await,
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (headers, bytes) = requests[0].split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("POST /4/worklogs HTTP/1.1\r\n"));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-tempo-token\r\n")
    );
    let body: Value = serde_json::from_str(bytes).unwrap();
    assert_eq!(body["authorAccountId"], "123456:synthetic-user");
    assert_eq!(body["issueId"], 19);
    assert_eq!(body["startDate"], "2026-10-16");
    assert_eq!(body["timeSpentSeconds"], 3600);
    assert_eq!(body["description"], "private synthetic work");
    assert_eq!(
        body["attributes"],
        json!([{"key":"_Account_","value":"SYNTHETIC"}])
    );
    assert_eq!(identity.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn lost_reply_stays_saved_and_neither_dispatch_nor_recovery_replays_it() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let identity = jira(3);
    let jira_client = Arc::new(identity.client());
    let service = TempoFixture::new(vec![String::new()]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        jira_client.clone(),
        &config,
        draft(),
    )
    .await
    .unwrap();
    let original = stored(&config).current().unwrap().to_vec();
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Unconfirmed && !report.acknowledgement_durable);
    assert_eq!(stored(&config).current(), Some(original.as_slice()));
    assert!(matches!(
        change.dispatch().await,
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    drop(change);
    let mut restored = SavedRecovery::open(&config, &jira_client)
        .await
        .unwrap()
        .unwrap();
    assert!(restored.phase() == WritePhase::Unconfirmed);
    let body: Value = serde_json::from_slice(&restored.draft().encode().unwrap()).unwrap();
    assert_eq!(body["description"], "private synthetic work");
    assert!(matches!(
        restored.cleanup(),
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    assert_eq!(service.requests.lock().unwrap().len(), 1);
    restored.discard().unwrap();
    assert!(stored(&config).current().is_none());
    assert_eq!(identity.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn denied_write_retains_the_draft_and_cannot_replay_the_rejected_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let identity = jira(2);
    let service = TempoFixture::new(vec![reply(403, "private response body must not escape")]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        draft(),
    )
    .await
    .unwrap();
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Rejected && report.acknowledgement_durable);
    assert_eq!(
        report.transport_problem,
        Some(TransportError::Authentication)
    );
    let body: Value = serde_json::from_slice(&change.draft().encode().unwrap()).unwrap();
    assert_eq!(body["description"], "private synthetic work");
    assert!(stored(&config).current().is_none());
    assert!(matches!(
        change.dispatch().await,
        Err(ChangeError::Domain(Error::WriteBlocked))
    ));
    assert_eq!(service.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn another_saved_change_blocks_capture_before_identity_or_mutation_requests() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut other = stored(&config);
    other.persist(b"preserve foreign pending bytes").unwrap();
    let identity = jira(0);
    let service = TempoFixture::new(vec![]);
    assert!(matches!(
        SavedChange::capture(
            Arc::new(service.client()),
            Arc::new(identity.client()),
            &config,
            draft()
        )
        .await,
        Err(ChangeError::Storage(StorageError::Pending))
    ));
    assert_eq!(
        stored(&config).current(),
        Some(b"preserve foreign pending bytes".as_slice())
    );
    assert!(
        identity.requests.lock().unwrap().is_empty() && service.requests.lock().unwrap().is_empty()
    );
}

#[tokio::test]
async fn changed_edit_baseline_keeps_recovery_and_never_sends_put() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let original = single_worklog();
    let baseline = parse_worklog(&serde_json::to_vec(&original).unwrap()).unwrap();
    let mut edit = Draft::edit(&baseline).unwrap();
    edit.set_duration(3600).unwrap();
    let mut changed = original;
    changed["description"] = json!("changed concurrently with identical timestamp");
    let identity = jira(2);
    let service = TempoFixture::new(vec![reply(200, &changed.to_string())]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        edit,
    )
    .await
    .unwrap();
    assert!(matches!(
        change.dispatch().await,
        Err(ChangeError::Domain(Error::ChangedWhileLoading))
    ));
    assert!(change.phase() == WritePhase::Ready && stored(&config).current().is_some());
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /4/worklogs/7 HTTP/1.1\r\n"));
}

#[tokio::test]
async fn update_rechecks_baseline_and_preserves_the_captured_optional_attributes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut original = single_worklog();
    original["attributes"]["values"] = json!([{"key":"_Custom_"}]);
    let baseline = parse_worklog(&serde_json::to_vec(&original).unwrap()).unwrap();
    let mut edit = Draft::edit(&baseline).unwrap();
    edit.set_duration(3600).unwrap();
    let mut updated = original.clone();
    updated["timeSpentSeconds"] = json!(3600);
    let identity = jira(2);
    let service = TempoFixture::new(vec![
        reply(200, &original.to_string()),
        reply(200, &updated.to_string()),
    ]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        edit,
    )
    .await
    .unwrap();
    assert!(change.dispatch().await.unwrap().phase == WritePhase::Confirmed);
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("PUT /4/worklogs/7 HTTP/1.1\r\n"));
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(body.get("issueId").is_none());
    assert_eq!(body["timeSpentSeconds"], 3600);
    assert_eq!(body["attributes"], json!([{"key":"_Custom_"}]));
}

#[tokio::test]
async fn delete_accepts_an_unchanged_unexpanded_baseline_and_sends_no_body() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut original = single_worklog();
    original["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("values");
    let baseline = parse_worklog(&serde_json::to_vec(&original).unwrap()).unwrap();
    let deletion = Draft::delete(&baseline).unwrap();
    let identity = jira(2);
    let service = TempoFixture::new(vec![reply(200, &original.to_string()), reply(204, "")]);
    let mut change = SavedChange::capture(
        Arc::new(service.client()),
        Arc::new(identity.client()),
        &config,
        deletion,
    )
    .await
    .unwrap();
    let report = change.dispatch().await.unwrap();
    assert!(report.phase == WritePhase::Confirmed);
    assert_eq!(report.worklog.unwrap().get(), 7);
    let requests = service.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("DELETE /4/worklogs/7 HTTP/1.1\r\n"));
    assert_eq!(requests[1].split_once("\r\n\r\n").unwrap().1, "");
}

#[tokio::test]
async fn malformed_success_and_rate_limit_remain_unconfirmed_without_replay() {
    for response in [reply(200, "{}"), reply(429, "private throttling body")] {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let identity = jira(2);
        let service = TempoFixture::new(vec![response]);
        let mut change = SavedChange::capture(
            Arc::new(service.client()),
            Arc::new(identity.client()),
            &config,
            draft(),
        )
        .await
        .unwrap();
        assert!(change.dispatch().await.unwrap().phase == WritePhase::Unconfirmed);
        assert!(stored(&config).current().is_some());
        assert!(matches!(
            change.dispatch().await,
            Err(ChangeError::Domain(Error::WriteBlocked))
        ));
        assert_eq!(service.requests.lock().unwrap().len(), 1);
    }
}
