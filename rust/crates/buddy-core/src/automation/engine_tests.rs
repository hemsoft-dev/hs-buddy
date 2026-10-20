use super::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Server {
    run: Run,
    claims: BTreeMap<String, Claim>,
    claim_calls: usize,
    finish_calls: usize,
    recover_calls: usize,
    lose_claim: bool,
    lose_finish: bool,
    uncommitted_claim: bool,
    tamper: bool,
    root: PathBuf,
}
#[derive(Clone)]
struct Service(Arc<Mutex<Server>>);

fn request_path(root: &Path) -> PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("native-automation-change-")
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
        })
        .expect("durable request before backend claim")
}

fn result() -> WorkerResult {
    WorkerResult {
        success: true,
        output: Some(json!({"stdout":"one execution 雪","stderr":""})),
        error: None,
        duration: 1.0,
    }
}

fn update_run(server: &mut Server, status: RunStatus, output: Option<Value>) {
    server.run.status = status;
    server.run.output = output.clone();
    for claim in server.claims.values_mut() {
        if let Claim::Claimed { run, .. } = claim {
            run.status = status;
            run.output = output.clone();
        }
    }
}

#[async_trait]
impl Queue for Service {
    fn deployment(&self) -> &str {
        "http://127.0.0.1:3210"
    }
    async fn recover_missed(&self) -> Result<(), BackendError> {
        self.0.lock().unwrap().recover_calls += 1;
        Ok(())
    }
    async fn pending(&self) -> Result<Option<Run>, BackendError> {
        let server = self.0.lock().unwrap();
        Ok((server.run.status == RunStatus::Pending).then(|| server.run.clone()))
    }
    async fn lookup(&self, request: &Request) -> Result<Option<Claim>, BackendError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .claims
            .get(&request.request_id)
            .cloned())
    }
    async fn claim(&self, request: &Request) -> Result<Claim, BackendError> {
        let mut server = self.0.lock().unwrap();
        server.claim_calls += 1;
        let path = request_path(&server.root);
        let bytes = std::fs::read(&path).unwrap();
        let stored = serde_json::Deserializer::from_slice(&bytes)
            .into_iter::<Value>()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(stored["request_id"], request.request_id);
        assert_eq!(stored["run_id"], request.run_id);
        if server.uncommitted_claim {
            server.uncommitted_claim = false;
            return Err(BackendError::UnknownWrite);
        }
        if let Some(original) = server.claims.get(&request.request_id) {
            return Ok(original.clone());
        }
        let claim = if server.run.status == RunStatus::Pending && server.run.id == request.run_id {
            server.run.status = RunStatus::Running;
            Claim::parse(json!({"kind":"claimed","requestId":request.request_id,
                "run":server.run,"job":{"_id":"job","name":"fixture","workerType":"exec","config":{"command":"fixture"}}}),
                &request.request_id,&request.run_id).unwrap()
        } else {
            Claim::Retired {
                request_id: request.request_id.clone(),
                run_id: request.run_id.clone(),
            }
        };
        server
            .claims
            .insert(request.request_id.clone(), claim.clone());
        if server.tamper {
            std::fs::write(path, b"{").unwrap();
        }
        if server.lose_claim {
            server.lose_claim = false;
            return Err(BackendError::UnknownWrite);
        }
        Ok(claim)
    }
    async fn run(&self, id: &str) -> Result<Option<Run>, BackendError> {
        let server = self.0.lock().unwrap();
        Ok((server.run.id == id).then(|| server.run.clone()))
    }
    async fn finish(&self, request: &Request, result: &WorkerResult) -> Result<(), BackendError> {
        let mut server = self.0.lock().unwrap();
        server.finish_calls += 1;
        assert!(server.claims.contains_key(&request.request_id));
        assert_eq!(server.run.id, request.run_id);
        let status = if server.run.status == RunStatus::Cancelled {
            RunStatus::Cancelled
        } else if result.success {
            RunStatus::Completed
        } else {
            RunStatus::Failed
        };
        update_run(&mut server, status, result.output.clone());
        if server.lose_finish {
            server.lose_finish = false;
            return Err(BackendError::UnknownWrite);
        }
        Ok(())
    }
    async fn cancel(&self, id: &str) -> Result<(), BackendError> {
        let mut server = self.0.lock().unwrap();
        assert_eq!(server.run.id, id);
        update_run(&mut server, RunStatus::Cancelled, None);
        Ok(())
    }
}

#[derive(Default)]
struct WorkerState {
    result: Option<WorkerResult>,
    cancelled: bool,
    account: Option<String>,
}
struct Handle(Arc<Mutex<WorkerState>>);
impl Worker for Handle {
    fn result(&self) -> Option<WorkerResult> {
        self.0.lock().unwrap().result.clone()
    }
    fn output(&self) -> (String, String) {
        ("live captured output".into(), String::new())
    }
    fn cancel(&self) {
        self.0.lock().unwrap().cancelled = true;
    }
}
#[derive(Clone, Default)]
struct Pool {
    starts: Arc<AtomicUsize>,
    state: Arc<Mutex<WorkerState>>,
}
impl Workers for Pool {
    fn start(&mut self, permit: Permit) -> Result<Box<dyn Worker>, WorkerResult> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        assert_eq!(permit.job.config.command.as_deref(), Some("fixture"));
        self.state.lock().unwrap().account = permit.request.context.account;
        Ok(Box::new(Handle(self.state.clone())))
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, Service, Pool) {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let run = serde_json::from_value(
        json!({"_id":"run-one","jobId":"job","status":"pending","startedAt":1.0}),
    )
    .unwrap();
    let server = Server {
        run,
        claims: BTreeMap::new(),
        claim_calls: 0,
        finish_calls: 0,
        recover_calls: 0,
        lose_claim: false,
        lose_finish: false,
        uncommitted_claim: false,
        tamper: false,
        root: root.path().to_owned(),
    };
    (
        root,
        config,
        Service(Arc::new(Mutex::new(server))),
        Pool::default(),
    )
}

#[tokio::test]
async fn lost_claim_and_result_acknowledgements_reconcile_without_another_execution() {
    let (_root, config, service, pool) = fixture();
    {
        let mut server = service.0.lock().unwrap();
        server.lose_claim = true;
        server.lose_finish = true;
    }
    let mut engine = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    let context = Context {
        account: Some("captured-account".into()),
        model: Some("captured-model".into()),
    };
    assert_eq!(
        engine.tick(context).await,
        Err(Error::Backend(BackendError::UnknownWrite))
    );
    let identity = engine.request().unwrap().request_id.clone();
    assert_eq!(
        engine.tick(Context::default()).await.unwrap(),
        Phase::Running
    );
    assert_eq!(pool.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        pool.state.lock().unwrap().account.as_deref(),
        Some("captured-account")
    );
    pool.state.lock().unwrap().result = Some(result());
    assert_eq!(
        engine.tick(Context::default()).await,
        Err(Error::Backend(BackendError::UnknownWrite))
    );
    drop(engine);
    let mut restored = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(restored.request().unwrap().request_id, identity);
    assert_eq!(
        restored.tick(Context::default()).await.unwrap(),
        Phase::Idle
    );
    assert!(restored.request().is_none());
    assert_eq!(pool.starts.load(Ordering::SeqCst), 1);
    let server = service.0.lock().unwrap();
    assert_eq!(server.claim_calls, 1);
    assert_eq!(server.finish_calls, 1);
    assert_eq!(server.run.status, RunStatus::Completed);
    assert_eq!(server.run.output, result().output);
}

#[tokio::test]
async fn restored_started_request_requires_attention_and_never_reexecutes() {
    let (_root, config, service, pool) = fixture();
    let mut engine = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(
        engine.tick(Context::default()).await.unwrap(),
        Phase::Running
    );
    let identity = engine.request().unwrap().request_id.clone();
    drop(engine);
    assert!(pool.state.lock().unwrap().cancelled);
    let mut restored = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(
        restored.tick(Context::default()).await.unwrap(),
        Phase::Attention
    );
    assert_eq!(restored.request().unwrap().request_id, identity);
    assert_eq!(pool.starts.load(Ordering::SeqCst), 1);
    restored.cancel().await.unwrap();
    assert_eq!(service.0.lock().unwrap().run.status, RunStatus::Cancelled);
    assert!(restored.request().is_none());
    assert_eq!(pool.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_restored_unconfirmed_claim_does_not_claim_pending_work_again() {
    let (_root, config, service, pool) = fixture();
    service.0.lock().unwrap().uncommitted_claim = true;
    let mut engine = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(
        engine.tick(Context::default()).await,
        Err(Error::Backend(BackendError::UnknownWrite))
    );
    drop(engine);
    let mut restored = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(
        restored.tick(Context::default()).await.unwrap(),
        Phase::Attention
    );
    assert_eq!(service.0.lock().unwrap().claim_calls, 1);
    assert_eq!(service.0.lock().unwrap().run.status, RunStatus::Pending);
    assert_eq!(pool.starts.load(Ordering::SeqCst), 0);
    restored.cancel().await.unwrap();
    assert!(restored.request().is_none());
    assert_eq!(pool.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn storage_change_between_claim_and_start_prevents_worker_dispatch() {
    let (root, config, service, pool) = fixture();
    service.0.lock().unwrap().tamper = true;
    let mut engine = Engine::open(service, pool.clone(), &config).unwrap();
    assert_eq!(
        engine.tick(Context::default()).await,
        Err(Error::Journal(JournalError::Storage(
            crate::pending_file::StorageError::Changed
        )))
    );
    assert_eq!(pool.starts.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(request_path(root.path())).unwrap(), b"{");
}

#[tokio::test]
async fn cancellation_stops_the_handle_and_saves_output_without_changing_terminal_status() {
    let (_root, config, service, pool) = fixture();
    let mut engine = Engine::open(service.clone(), pool.clone(), &config).unwrap();
    assert_eq!(
        engine.tick(Context::default()).await.unwrap(),
        Phase::Running
    );
    engine.cancel().await.unwrap();
    assert!(pool.state.lock().unwrap().cancelled);
    let mut cancelled = result();
    cancelled.success = false;
    cancelled.error = Some("Cancelled by user".into());
    pool.state.lock().unwrap().result = Some(cancelled);
    assert_eq!(
        engine.tick(Context::default()).await.unwrap(),
        Phase::Reconciling
    );
    assert_eq!(engine.tick(Context::default()).await.unwrap(), Phase::Idle);
    let server = service.0.lock().unwrap();
    assert_eq!(server.run.status, RunStatus::Cancelled);
    assert_eq!(server.run.output, result().output);
    assert_eq!(server.finish_calls, 1);
    assert_eq!(pool.starts.load(Ordering::SeqCst), 1);
}
