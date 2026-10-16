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
    tokio::time::timeout(Duration::from_secs(20), handle.retry_save().unwrap())
        .await
        .unwrap();
    let state = handle.snapshot();
    assert_eq!(state.save, SaveState::Unconfirmed);
    assert!(!state.busy);
    assert!(state.output.is_empty());
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
