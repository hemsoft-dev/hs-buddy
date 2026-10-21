use super::*;
use std::sync::Mutex;
struct Server {
    calls: Mutex<Vec<Value>>,
    result: Result<Value, BackendError>,
}
#[async_trait]
impl Service for Server {
    fn deployment(&self) -> &str {
        "http://127.0.0.1:3210"
    }
    async fn send(&self, _: ChangeKind, arguments: Value) -> Result<Value, BackendError> {
        self.calls.lock().unwrap().push(arguments);
        self.result.clone()
    }
}
fn path(root: &Path) -> std::path::PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("native-automation-edit-change-")
                && path.extension().is_some_and(|ext| ext == "json")
        })
        .unwrap()
}
#[tokio::test]
async fn a_lost_edit_reply_retains_exact_configuration_and_never_resends_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server {
        calls: Mutex::default(),
        result: Err(BackendError::UnknownWrite),
    };
    let args = serde_json::json!({"name":"fixture 雪","config":{"command":"fixture","env":{"VALUE":"雪"},"future":false}});
    let mut edit = Edit::open(&config, service.deployment()).unwrap();
    edit.prepare(ChangeKind::CreateJob, args.clone()).unwrap();
    let original = std::fs::read(path(root.path())).unwrap();
    assert!(edit.dispatch(&service).await.is_err());
    assert!(edit.dispatch(&service).await.is_err());
    drop(edit);
    let mut restored = Edit::open(&config, service.deployment()).unwrap();
    assert_eq!(restored.capture().unwrap().arguments, args);
    assert!(restored.dispatch(&service).await.is_err());
    assert!(restored.clear_acknowledged().is_err());
    assert_eq!(std::fs::read(path(root.path())).unwrap(), original);
    assert_eq!(*service.calls.lock().unwrap(), vec![args]);
}
#[tokio::test]
async fn acknowledgement_survives_restart_and_changed_files_are_not_discarded() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server {
        calls: Mutex::default(),
        result: Ok(Value::String("job-id".into())),
    };
    let mut edit = Edit::open(&config, service.deployment()).unwrap();
    edit.prepare(
        ChangeKind::UpdateJob,
        serde_json::json!({"id":"job-id","name":"new"}),
    )
    .unwrap();
    assert_eq!(edit.dispatch(&service).await.unwrap(), "job-id");
    drop(edit);
    let mut restored = Edit::open(&config, service.deployment()).unwrap();
    assert!(restored.acknowledged());
    assert!(restored.dispatch(&service).await.is_err());
    let file = path(root.path());
    std::fs::write(&file, b"{").unwrap();
    assert!(restored.clear_acknowledged().is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"{");
    assert_eq!(service.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn unsupported_success_responses_keep_the_capture_and_backend_changes_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    let service = Server {
        calls: Mutex::default(),
        result: Ok(Value::Null),
    };
    let mut edit = Edit::open(&config, service.deployment()).unwrap();
    edit.prepare(
        ChangeKind::CreateSchedule,
        serde_json::json!({"name":"schedule"}),
    )
    .unwrap();
    let original = std::fs::read(path(root.path())).unwrap();
    assert!(matches!(
        edit.dispatch(&service).await,
        Err(Error::Backend(BackendError::UnknownWrite))
    ));
    assert!(!edit.acknowledged());
    assert!(Edit::open(&config, "http://127.0.0.1:3211").is_err());
    assert_eq!(std::fs::read(path(root.path())).unwrap(), original);
    edit.discard_explicitly().unwrap();
    assert!(edit.capture().is_none());
}
