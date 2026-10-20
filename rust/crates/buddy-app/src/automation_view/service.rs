//! Serialized background I/O and execution; GPUI only reads bounded snapshots.
use crate::runtime::Runtime;
use buddy_core::automation::{
    backend::{AutomationBackend, ChangeKind},
    edit::Edit,
    engine::{Engine, Phase},
    journal::Context,
    models::{Job, Run, Schedule},
    trigger::{Capture as TriggerCapture, Receipt, Trigger},
    workers::NativeWorkers,
};
use futures::channel::oneshot;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub jobs: Vec<Job>,
    pub schedules: Vec<Schedule>,
    pub runs: Vec<Run>,
    pub phase: Phase,
    pub active_run: Option<String>,
    pub output: Option<(String, String)>,
    pub edit: Option<buddy_core::automation::edit::Capture>,
    pub edit_acknowledged: bool,
    pub trigger: Option<TriggerCapture>,
    pub trigger_receipt: Option<Receipt>,
    pub loaded: bool,
    pub connected: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub sequence: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            jobs: vec![],
            schedules: vec![],
            runs: vec![],
            phase: Phase::Idle,
            active_run: None,
            output: None,
            edit: None,
            edit_acknowledged: false,
            trigger: None,
            trigger_receipt: None,
            loaded: false,
            connected: false,
            error: None,
            notice: None,
            sequence: 0,
        }
    }
}
pub(super) enum Command {
    Refresh,
    Context(Context),
    Save(ChangeKind, Value),
    Run(String, Option<Value>),
    ReconcileTrigger,
    RetryTrigger,
    ClearTrigger,
    ClearEdit,
    DiscardEdit,
    Cancel(String),
    Stop,
}
struct Envelope {
    command: Command,
    reply: Option<oneshot::Sender<Result<(), String>>>,
}
#[derive(Clone)]
pub(super) struct Handle {
    sender: mpsc::UnboundedSender<Envelope>,
    state: Arc<Mutex<Snapshot>>,
}
impl Handle {
    pub fn start(runtime: &Runtime, config: PathBuf, context: Context) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let handle = Self {
            sender,
            state: state.clone(),
        };
        runtime.spawn_detached(async move {
            match Coordinator::open(&config, context) {
                Ok(coordinator) => coordinator.run(receiver, state).await,
                Err(error) => {
                    let mut snapshot = state.lock().unwrap_or_else(|e| e.into_inner());
                    snapshot.error = Some(error);
                    snapshot.phase = Phase::Attention;
                    snapshot.sequence += 1;
                }
            }
        });
        handle
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn send(&self, command: Command) -> bool {
        self.sender
            .send(Envelope {
                command,
                reply: None,
            })
            .is_ok()
    }
    pub fn request(&self, command: Command) -> oneshot::Receiver<Result<(), String>> {
        let (reply, receiver) = oneshot::channel();
        let _ = self.sender.send(Envelope {
            command,
            reply: Some(reply),
        });
        receiver
    }
}

struct Coordinator {
    backend: AutomationBackend,
    engine: Engine<AutomationBackend, NativeWorkers>,
    edit: Edit,
    trigger: Trigger,
    context: Context,
    snapshot: Snapshot,
}
impl Coordinator {
    fn open(config: &std::path::Path, context: Context) -> Result<Self, String> {
        let backend = AutomationBackend::new(&buddy_core::convex_data::convex_url())
            .map_err(|e| e.to_string())?;
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("The home directory is unavailable.")?;
        let owned = config
            .parent()
            .ok_or("The configuration profile is unavailable.")?
            .join("native-automation-runtime");
        let workers = NativeWorkers {
            executable: std::env::current_exe()
                .map_err(|_| "The worker executable is unavailable.")?,
            owned_directory: owned,
            skills_directory: home.join(".agents/skills"),
        };
        let engine = Engine::open(backend.clone(), workers, config).map_err(|e| e.to_string())?;
        let edit = Edit::open(config, backend.deployment()).map_err(|e| e.to_string())?;
        let trigger = Trigger::open(config, backend.deployment()).map_err(|e| e.to_string())?;
        Ok(Self {
            backend,
            engine,
            edit,
            trigger,
            context,
            snapshot: Snapshot::default(),
        })
    }
    fn publish(&mut self, target: &Mutex<Snapshot>) {
        self.snapshot.phase = self.engine.phase();
        self.snapshot.active_run = self.engine.request().map(|request| request.run_id.clone());
        self.snapshot.output = self.engine.output();
        self.snapshot.edit = self.edit.capture().cloned();
        self.snapshot.edit_acknowledged = self.edit.acknowledged();
        self.snapshot.trigger = self.trigger.capture().cloned();
        self.snapshot.trigger_receipt = self.trigger.receipt().cloned();
        self.snapshot.sequence = self.snapshot.sequence.wrapping_add(1);
        *target.lock().unwrap_or_else(|e| e.into_inner()) = self.snapshot.clone();
    }
    async fn refresh(&mut self) {
        let (jobs, schedules, runs) = tokio::join!(
            self.backend.jobs(),
            self.backend.schedules(),
            self.backend.recent_runs()
        );
        match (jobs, schedules, runs) {
            (Ok(jobs), Ok(schedules), Ok(runs)) => {
                self.snapshot.jobs = jobs;
                self.snapshot.schedules = schedules;
                self.snapshot.runs = runs;
                self.snapshot.connected = true;
                self.snapshot.loaded = true;
            }
            (jobs, schedules, runs) => {
                self.snapshot.connected = false;
                self.snapshot.error = jobs
                    .err()
                    .or(schedules.err())
                    .or(runs.err())
                    .map(|e| e.to_string());
            }
        }
    }
    async fn command(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::Context(context) => {
                self.context = context;
                return Ok(());
            }
            Command::Refresh => {
                self.snapshot.error = None;
                self.refresh().await;
                return Ok(());
            }
            Command::Save(kind, arguments) => {
                self.edit
                    .prepare(kind, arguments)
                    .map_err(|e| e.to_string())?;
                self.edit
                    .dispatch(&self.backend)
                    .await
                    .map_err(|e| e.to_string())?;
                self.snapshot.notice = Some("Saved. Refreshing automation…".into());
                self.edit.clear_acknowledged().map_err(|e| e.to_string())?;
            }
            Command::Run(id, input) => {
                self.trigger
                    .prepare(&id, input)
                    .map_err(|e| e.to_string())?;
                self.trigger
                    .dispatch(&self.backend)
                    .await
                    .map_err(|e| e.to_string())?;
                self.snapshot.notice =
                    Some("Run queued once. The dispatcher will claim it.".into());
                self.trigger
                    .clear_acknowledged()
                    .map_err(|e| e.to_string())?;
            }
            Command::RetryTrigger => {
                self.trigger
                    .retry_captured(&self.backend)
                    .await
                    .map_err(|e| e.to_string())?;
                self.snapshot.notice = Some("The original run request is confirmed.".into());
                self.trigger
                    .clear_acknowledged()
                    .map_err(|e| e.to_string())?;
            }
            Command::ReconcileTrigger => {
                let receipt = self
                    .trigger
                    .reconcile(&self.backend)
                    .await
                    .map_err(|e| e.to_string())?;
                self.snapshot.notice = Some(if receipt.is_some() { "The original run request is confirmed." } else { "No receipt was found. The saved request remains available for explicit retry." }.into());
                if receipt.is_some() {
                    self.trigger
                        .clear_acknowledged()
                        .map_err(|e| e.to_string())?;
                }
            }
            Command::ClearTrigger => self
                .trigger
                .clear_acknowledged()
                .map_err(|e| e.to_string())?,
            Command::ClearEdit => self.edit.clear_acknowledged().map_err(|e| e.to_string())?,
            Command::DiscardEdit => {
                self.edit.discard_explicitly().map_err(|e| e.to_string())?;
                self.snapshot.notice =
                    Some("The local capture was discarded. Remote changes were not undone.".into());
            }
            Command::Cancel(id) => {
                if self
                    .engine
                    .request()
                    .is_some_and(|request| request.run_id == id)
                {
                    self.engine.cancel().await.map_err(|e| e.to_string())?;
                } else {
                    self.backend
                        .change(ChangeKind::CancelRun, serde_json::json!({"id":id}))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                self.snapshot.notice =
                    Some("Cancellation requested. Waiting for the owned worker to stop.".into());
            }
            Command::Stop => return Ok(()),
        }
        self.refresh().await;
        Ok(())
    }
    async fn run(
        mut self,
        mut receiver: mpsc::UnboundedReceiver<Envelope>,
        target: Arc<Mutex<Snapshot>>,
    ) {
        self.refresh().await;
        self.publish(&target);
        let mut ticks = tokio::time::interval(Duration::from_secs(2));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut polls = 0;
        loop {
            tokio::select! {
                command = receiver.recv() => {
                    let Some(envelope) = command else { break; };
                    if matches!(envelope.command, Command::Stop) { break; }
                    self.snapshot.error = None;
                    let result = self.command(envelope.command).await;
                    if let Err(error) = &result { self.snapshot.error = Some(error.clone()); }
                    self.publish(&target);
                    if let Some(reply) = envelope.reply { let _ = reply.send(result); }
                }
                _ = ticks.tick() => {
                    let before = self.engine.phase();
                    if let Err(error) = self.engine.tick(self.context.clone()).await { self.snapshot.error = Some(error.to_string()); }
                    polls += 1;
                    // A claimed, finished or cancelled run changes History at once.
                    if polls % 5 == 0 || self.engine.phase() != before { self.refresh().await; }
                }
            }
            self.publish(&target);
        }
    }
}
