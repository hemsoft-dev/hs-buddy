use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    receipts: BTreeMap<String, Receipt>,
    calls: usize,
    reads: usize,
    lost: bool,
    wrong: bool,
}
struct Server {
    state: Mutex<State>,
    root: std::path::PathBuf,
}
impl Server {
    fn new(root: &Path) -> Self {
        Self {
            state: Mutex::new(State::default()),
            root: root.into(),
        }
    }
}
fn checkpoint(root: &Path) -> std::path::PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("native-automation-trigger-change-")
                && path.extension().is_some_and(|ext| ext == "json")
        })
        .expect("captured request file")
}
#[async_trait]
impl Service for Server {
    fn deployment(&self) -> &str {
        "http://127.0.0.1:3210"
    }
    async fn send(&self, capture: &Capture) -> Result<Receipt, BackendError> {
        let bytes = std::fs::read(checkpoint(&self.root)).unwrap();
        let persisted: Capture = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(persisted.request_id, capture.request_id);
        assert_eq!(persisted.job_id, capture.job_id);
        assert_eq!(persisted.input, capture.input);
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        if state.wrong {
            return Ok(Receipt {
                kind: Outcome::Recorded,
                request_id: uuid::Uuid::new_v4().to_string(),
                run_id: "wrong".into(),
            });
        }
        let receipt = state
            .receipts
            .entry(capture.request_id.clone())
            .or_insert_with(|| Receipt {
                kind: Outcome::Recorded,
                request_id: capture.request_id.clone(),
                run_id: "one-run".into(),
            })
            .clone();
        if state.lost {
            state.lost = false;
            return Err(BackendError::UnknownWrite);
        }
        Ok(receipt)
    }
    async fn lookup(&self, id: &str) -> Result<Option<Receipt>, BackendError> {
        let mut state = self.state.lock().unwrap();
        state.reads += 1;
        Ok(state.receipts.get(id).cloned())
    }
}

#[tokio::test]
async fn a_lost_reply_restores_exact_null_input_and_reconciles_without_another_send() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server::new(root.path());
    service.state.lock().unwrap().lost = true;
    let mut trigger = Trigger::open(&config, service.deployment()).unwrap();
    trigger.prepare("job", Some(Value::Null)).unwrap();
    let original = std::fs::read(checkpoint(root.path())).unwrap();
    assert!(matches!(
        trigger.dispatch(&service).await,
        Err(Error::Backend(BackendError::UnknownWrite))
    ));
    assert!(matches!(
        trigger.dispatch(&service).await,
        Err(Error::Restored)
    ));
    drop(trigger);
    let mut restored = Trigger::open(&config, service.deployment()).unwrap();
    assert_eq!(restored.capture().unwrap().input, Some(Value::Null));
    assert_eq!(std::fs::read(checkpoint(root.path())).unwrap(), original);
    assert!(matches!(
        restored.dispatch(&service).await,
        Err(Error::Restored)
    ));
    assert_eq!(
        restored.reconcile(&service).await.unwrap().unwrap().run_id,
        "one-run"
    );
    let acknowledged = std::fs::read(checkpoint(root.path())).unwrap();
    drop(restored);
    let mut restored = Trigger::open(&config, service.deployment()).unwrap();
    assert!(restored.receipt().is_some());
    assert_eq!(
        std::fs::read(checkpoint(root.path())).unwrap(),
        acknowledged
    );
    restored.clear_acknowledged().unwrap();
    assert!(restored.capture().is_none());
    assert_eq!(service.state.lock().unwrap().calls, 1);
    assert_eq!(service.state.lock().unwrap().receipts.len(), 1);
}

#[tokio::test]
async fn an_explicit_retry_uses_the_original_capture_and_never_creates_a_second_run() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server::new(root.path());
    service.state.lock().unwrap().lost = true;
    let mut trigger = Trigger::open(&config, service.deployment()).unwrap();
    trigger
        .prepare("job", Some(serde_json::json!({"text":"雪","value":false})))
        .unwrap();
    let id = trigger.capture().unwrap().request_id.clone();
    assert!(trigger.dispatch(&service).await.is_err());
    assert!(matches!(
        trigger.prepare("other", None),
        Err(Error::Pending)
    ));
    assert_eq!(
        trigger.retry_captured(&service).await.unwrap().request_id,
        id
    );
    assert!(trigger.retry_captured(&service).await.is_err());
    let state = service.state.lock().unwrap();
    assert_eq!(state.calls, 2);
    assert_eq!(state.receipts.len(), 1);
}

#[tokio::test]
async fn changed_files_wrong_receipts_and_backend_changes_preserve_the_capture() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server::new(root.path());
    service.state.lock().unwrap().wrong = true;
    let mut trigger = Trigger::open(&config, service.deployment()).unwrap();
    trigger.prepare("job", None).unwrap();
    let path = checkpoint(root.path());
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        trigger.dispatch(&service).await,
        Err(Error::Invalid)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(Trigger::open(&config, "http://127.0.0.1:3211").is_err());
    std::fs::write(&path, b"{").unwrap();
    assert!(matches!(
        trigger.retry_captured(&service).await,
        Err(Error::Storage(StorageError::Changed))
    ));
    assert!(trigger.clear_acknowledged().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{");
    assert_eq!(service.state.lock().unwrap().calls, 1);
}

#[test]
fn oversized_input_is_rejected_before_persistence_or_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let mut trigger = Trigger::open(&config, "http://127.0.0.1:3210").unwrap();
    assert!(matches!(
        trigger.prepare("job", Some(Value::String("x".repeat(MAX_BYTES)))),
        Err(Error::Storage(StorageError::TooLarge))
    ));
    assert!(trigger.capture().is_none());
    assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|ext| ext != "json")
    }));
}
