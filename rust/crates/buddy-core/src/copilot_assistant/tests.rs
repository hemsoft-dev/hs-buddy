use super::*;

fn options(root: PathBuf) -> RunOptions {
    RunOptions {
        context: Context {
            account: "synthetic-account".into(),
            model: "gpt-4.1".into(),
            review: None,
        },
        prompt: "Synthetic prompt 界".into(),
        saved: false,
        review: None,
        backend: None,
        executable: root.join("must-not-start.exe"),
        owned_directory: root,
    }
}

#[test]
fn cancelling_before_poll_never_prepares_a_runtime_or_reads_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let (handle, future) = start(options(root.clone())).unwrap();
    assert!(handle.cancel());
    assert!(!handle.cancel());
    tokio::runtime::Runtime::new().unwrap().block_on(future);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.phase, Phase::Cancelled);
    assert_eq!(snapshot.save, SaveState::NotRequested);
    assert_eq!(snapshot.progress, Progress::Finished);
    assert!(!snapshot.busy);
    assert!(snapshot.output.is_empty());
    assert!(!root.exists());
    assert!(handle.retry_save().is_none());
}

#[test]
fn dropping_the_handle_aborts_even_an_unpolled_turn() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let (handle, future) = start(options(root.clone())).unwrap();
    drop(handle);
    tokio::runtime::Runtime::new().unwrap().block_on(future);
    assert!(!root.exists());
}

#[test]
fn saved_turns_require_a_backend_before_any_background_work_exists() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let mut input = options(root.clone());
    input.saved = true;
    assert!(matches!(start(input), Err(Failure::PersistenceFailed)));
    assert!(!root.exists());
}

#[test]
fn invalid_context_is_rejected_without_exposing_private_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let mut input = options(root.clone());
    input.context.account = "bad--account".into();
    assert!(matches!(start(input), Err(Failure::InvalidContext)));
    assert!(!root.exists());
}

#[tokio::test]
async fn unavailable_saved_begin_and_storage_reconciliation_never_prepare_or_send() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut input = options(root.clone());
    input.saved = true;
    input.backend = Some(CopilotBackend::new(&format!("http://127.0.0.1:{port}")).unwrap());
    let (handle, future) = start(input).unwrap();
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .unwrap();
    let state = handle.snapshot();
    assert_eq!(state.save, SaveState::Unconfirmed);
    assert!(!state.busy);
    assert!(state.output.is_empty());
    assert!(state.result_id.is_none());
    assert!(!root.exists());
    let retry = handle.retry_save().unwrap();
    assert!(handle.snapshot().busy);
    assert_eq!(handle.snapshot().save, SaveState::Saving);
    assert!(handle.retry_save().is_none());
    tokio::time::timeout(Duration::from_secs(20), retry)
        .await
        .unwrap();
    let state = handle.snapshot();
    assert_eq!(state.save, SaveState::Unconfirmed);
    assert!(!state.busy);
    assert!(state.output.is_empty());
    assert!(!root.exists());
}

#[tokio::test]
async fn a_reconciled_pending_begin_retains_an_identical_terminal_failure_for_storage_retry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut input = options(root.clone());
    input.saved = true;
    input.backend = Some(CopilotBackend::new(&format!("http://127.0.0.1:{port}")).unwrap());
    let (handle, future) = start(input).unwrap();
    drop(future);
    {
        let mut state = lock(&handle.shared.state);
        let key = state.execution.key();
        assert!(state.execution.record_unknown(key));
        state.save = SaveState::Unconfirmed;
        state.busy = false;
    }
    let captured = reconciled_record(
        &handle.shared,
        "pending-result".into(),
        RecordStatus::Pending,
    )
    .expect("an unstarted pending record must be closed");
    assert_eq!(captured.id, "pending-result");
    assert!(captured.output.is_empty());
    assert_eq!(captured.failure, Some("delivery-unknown"));
    assert_eq!(
        handle.snapshot().result_id.as_deref(),
        Some("pending-result")
    );
    let retry = handle.retry_save().unwrap();
    assert!(handle.snapshot().busy);
    assert!(handle.retry_save().is_none());
    tokio::time::timeout(Duration::from_secs(20), retry)
        .await
        .unwrap();
    let state = handle.snapshot();
    assert_eq!(state.save, SaveState::Unconfirmed);
    assert!(!state.busy);
    assert!(state.output.is_empty());
    assert!(Arc::ptr_eq(
        &captured,
        lock(&handle.shared.state).captured.as_ref().unwrap()
    ));
    assert!(!root.exists());
}

#[test]
fn reconciliation_never_rewrites_an_already_running_or_terminal_result() {
    for status in [
        RecordStatus::Running,
        RecordStatus::Completed,
        RecordStatus::Failed,
        RecordStatus::Deleted,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("must-remain-absent");
        let mut input = options(root.clone());
        input.saved = true;
        input.backend = Some(CopilotBackend::new("http://127.0.0.1:1").unwrap());
        let (handle, future) = start(input).unwrap();
        drop(future);
        {
            let mut state = lock(&handle.shared.state);
            let key = state.execution.key();
            assert!(state.execution.record_unknown(key));
        }
        assert!(reconciled_record(&handle.shared, "existing-result".into(), status).is_none());
        assert!(lock(&handle.shared.state).captured.is_none());
        assert_eq!(handle.snapshot().save, SaveState::Reconciled);
        assert!(!root.exists());
    }
}

#[tokio::test]
async fn an_in_flight_storage_retry_keeps_the_captured_answer_visible_and_blocks_replacement() {
    use tokio::io::AsyncReadExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-remain-absent");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrived, incoming) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 1024];
        assert!(
            tokio::time::timeout(Duration::from_secs(5), stream.read(&mut request))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        arrived.send(()).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;
    });
    let mut input = options(root.clone());
    input.saved = true;
    input.backend = Some(CopilotBackend::new(&format!("http://{address}")).unwrap());
    let (handle, future) = start(input).unwrap();
    drop(future);
    {
        let mut state = lock(&handle.shared.state);
        let key = state.execution.key();
        assert!(state.execution.record_created(key, "saved-answer".into()));
        state.execution.begin_send().unwrap();
        assert!(
            state
                .execution
                .final_message(key, "answer-message", "Captured answer 界")
        );
        assert!(state.execution.idle(key));
        assert!(state.execution.persistence_failed(key));
        state.captured = Some(Arc::new(CapturedOutcome {
            id: "saved-answer".into(),
            output: "Captured answer 界".into(),
            failure: None,
        }));
        state.save = SaveState::Unconfirmed;
        state.busy = false;
    }
    let retry = tokio::spawn(handle.retry_save().unwrap());
    tokio::time::timeout(Duration::from_secs(5), incoming)
        .await
        .unwrap()
        .unwrap();
    let state = handle.snapshot();
    assert!(state.busy);
    assert_eq!(state.save, SaveState::Saving);
    assert_eq!(state.output, "Captured answer 界");
    assert_eq!(state.result_id.as_deref(), Some("saved-answer"));
    assert!(handle.retry_save().is_none());
    release.send(()).unwrap();
    server.await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), retry)
        .await
        .unwrap()
        .unwrap();
    let state = handle.snapshot();
    assert!(!state.busy);
    assert_eq!(state.save, SaveState::Unconfirmed);
    assert_eq!(state.output, "Captured answer 界");
    assert!(!root.exists());
}

#[cfg(windows)]
#[test]
fn temporary_state_cleanup_waits_for_a_releasing_windows_file_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    let parent = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir_in(parent.path()).unwrap();
    let path = directory.path().to_owned();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(path.join("synthetic-state"))
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        drop(file);
    });
    assert!(close_temporary_directory(directory));
    release.join().unwrap();
    assert!(!path.exists());
}
