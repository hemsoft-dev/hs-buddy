//! Serialized background I/O and execution; GPUI only reads bounded snapshots.
use crate::runtime::Runtime;
use buddy_core::automation::{
    backend::{AutomationBackend, ChangeKind, Cleanup, Error as BackendError},
    edit::{Edit, Error as EditError},
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

#[derive(Clone, PartialEq)]
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
    /// The last failed list refresh; cleared when all three reads succeed.
    pub connection_error: Option<String>,
    /// The last failed dispatcher tick; cleared by the next successful tick.
    pub dispatch_error: Option<String>,
    /// The last failed user command; cleared when the next command starts.
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The open schedule detail's newest runs (`runs:listBySchedule`).
    pub schedule_runs: Option<ScheduleRuns>,
    pub sequence: u64,
}

/// `ScheduleDetailPanel` asks for this many recent runs.
pub(super) const SCHEDULE_RUNS: u32 = 10;
/// `RunList` deletes finished runs older than this many days.
pub(super) const CLEANUP_DAYS: u32 = 7;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ScheduleRuns {
    pub schedule_id: String,
    /// `None` until the first read for this schedule answers.
    pub runs: Option<Vec<Run>>,
    /// The last failed read; earlier runs stay listed.
    pub error: Option<String>,
}

/// What a sent cleanup says. It is sent once: an unknown outcome is reported,
/// never retried, and History's next refresh shows the runs that remain.
pub(super) fn cleanup_outcome(result: Result<Cleanup, BackendError>) -> Result<String, String> {
    match result {
        Ok(Cleanup { deleted, more }) => Ok(format!(
            "Cleaned up {deleted} old run{}.{}",
            if deleted == 1 { "" } else { "s" },
            if more {
                " Older runs remain; clean up again to continue."
            } else {
                ""
            }
        )),
        Err(BackendError::Unavailable) => {
            Err("Automation is offline. No runs were deleted.".into())
        }
        Err(BackendError::Rejected) => {
            Err("The backend rejected the cleanup. No runs were deleted.".into())
        }
        Err(BackendError::UnknownWrite | BackendError::InvalidResponse) => Err(
            "The cleanup result is unknown. It was sent once and is not sent again; History shows the runs that remain."
                .into(),
        ),
    }
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
            connection_error: None,
            dispatch_error: None,
            error: None,
            notice: None,
            schedule_runs: None,
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
    DiscardTrigger,
    DismissNotice,
    ClearEdit,
    DiscardEdit,
    Cancel(String),
    /// Follow this schedule's recent runs, or none.
    ScheduleRuns(Option<String>),
    /// `runs:cleanupNative` for runs older than [`CLEANUP_DAYS`], after confirmation.
    Cleanup,
    Stop,
}
pub(super) struct Envelope {
    pub(super) command: Command,
    reply: Option<oneshot::Sender<Result<(), String>>>,
}
/// Resolves after the coordinator task's future has finished, so the consumed coordinator and
/// its storage guards are already dropped. `Canceled` means the task unwound or was dropped.
pub(super) type Completion = oneshot::Receiver<()>;
#[derive(Clone)]
pub(super) struct Handle {
    sender: mpsc::UnboundedSender<Envelope>,
    state: Arc<Mutex<Snapshot>>,
}
impl Handle {
    pub fn start(runtime: &Runtime, config: PathBuf, context: Context) -> (Self, Completion) {
        Self::spawn(runtime, move |receiver, state| async move {
            match Coordinator::open(&config, context) {
                Ok(coordinator) => coordinator.run(receiver, state).await,
                Err(error) => {
                    let mut snapshot = state.lock().unwrap_or_else(|e| e.into_inner());
                    snapshot.error = Some(error);
                    snapshot.phase = Phase::Attention;
                    snapshot.sequence += 1;
                }
            }
        })
    }
    /// Starts `task` as the owner of the command queue and the snapshot.
    pub(super) fn spawn<F, Fut>(runtime: &Runtime, task: F) -> (Self, Completion)
    where
        F: FnOnce(mpsc::UnboundedReceiver<Envelope>, Arc<Mutex<Snapshot>>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (sender, receiver) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let handle = Self {
            sender,
            state: state.clone(),
        };
        let completion = runtime.spawn(task(receiver, state));
        (handle, completion)
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

/// A signed-in result is reused for five minutes; a signed-out one is retried after
/// 30 seconds, so a new login is picked up soon.
pub(super) fn lookup_due(
    cached: Option<&(Option<String>, std::time::Instant)>,
    now: std::time::Instant,
) -> bool {
    cached.is_none_or(|(account, at)| {
        now.duration_since(*at) >= Duration::from_secs(if account.is_some() { 300 } else { 30 })
    })
}

/// Advances the sequence only when the content changed. The view redraws on a new
/// sequence, so an idle two-second tick must not repaint the whole window.
pub(super) fn publish_into(target: &Mutex<Snapshot>, mut next: Snapshot) -> bool {
    let mut current = target.lock().unwrap_or_else(|e| e.into_inner());
    next.sequence = current.sequence;
    if *current == next {
        return false;
    }
    next.sequence = current.sequence.wrapping_add(1);
    *current = next;
    true
}

/// A confirmed change reports what it did, in the past tense.
pub(super) fn save_notice(kind: ChangeKind, arguments: &Value) -> &'static str {
    match kind {
        ChangeKind::CreateJob => "Job created.",
        ChangeKind::UpdateJob => "Job saved.",
        ChangeKind::DeleteJob => "Job deleted.",
        ChangeKind::CreateSchedule => "Schedule created.",
        ChangeKind::UpdateSchedule => match (
            arguments.as_object().map(|object| object.len()),
            arguments.get("enabled"),
        ) {
            (Some(2), Some(Value::Bool(false))) => "Schedule paused.",
            (Some(2), Some(Value::Bool(true))) => "Schedule enabled.",
            _ => "Schedule saved.",
        },
        ChangeKind::DeleteSchedule => "Schedule deleted.",
        ChangeKind::CancelRun => "Cancellation requested.",
    }
}

struct Coordinator {
    backend: AutomationBackend,
    engine: Engine<AutomationBackend, NativeWorkers>,
    edit: Edit,
    trigger: Trigger,
    context: Context,
    /// The `gh` CLI's active account, used when settings leave the account empty.
    active_account: Option<(Option<String>, std::time::Instant)>,
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
            active_account: None,
            snapshot: Snapshot::default(),
        })
    }
    fn publish(&mut self, target: &Mutex<Snapshot>) {
        self.snapshot.phase = self.engine.phase();
        self.snapshot.active_run = self.engine.request().map(|request| request.run_id.clone());
        self.snapshot.output = super::render::live_output(
            self.engine.request().map(|request| request.run_id.clone()),
            self.engine.output(),
        );
        self.snapshot.edit = self.edit.capture().cloned();
        self.snapshot.edit_acknowledged = self.edit.acknowledged();
        self.snapshot.trigger = self.trigger.capture().cloned();
        self.snapshot.trigger_receipt = self.trigger.receipt().cloned();
        publish_into(target, self.snapshot.clone());
    }
    /// An empty account setting means the active `gh` account, as in chat and model
    /// discovery. Resolve it (at most every five minutes; `gh auth status` checks online) so each run captures a real login.
    async fn dispatch_context(&mut self) -> Context {
        let mut context = self.context.clone();
        if context.account.is_none() {
            if lookup_due(self.active_account.as_ref(), std::time::Instant::now()) {
                let account = buddy_core::gh::active_account().await;
                self.active_account = Some((account, std::time::Instant::now()));
            }
            context.account = self
                .active_account
                .as_ref()
                .and_then(|(account, _)| account.clone());
        }
        context
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
                self.snapshot.connection_error = None;
                self.refresh_schedule_runs().await;
            }
            (jobs, schedules, runs) => {
                self.snapshot.connected = false;
                self.snapshot.connection_error = jobs
                    .err()
                    .or(schedules.err())
                    .or(runs.err())
                    .map(|e| e.to_string());
            }
        }
    }
    async fn refresh_schedule_runs(&mut self) {
        let Some(id) = self
            .snapshot
            .schedule_runs
            .as_ref()
            .map(|s| s.schedule_id.clone())
        else {
            return;
        };
        let result = self.backend.schedule_runs(&id, SCHEDULE_RUNS).await;
        // A newer selection may have replaced this one while the read was out.
        if let Some(current) = self
            .snapshot
            .schedule_runs
            .as_mut()
            .filter(|current| current.schedule_id == id)
        {
            match result {
                Ok(runs) => {
                    current.runs = Some(runs);
                    current.error = None;
                }
                Err(error) => current.error = Some(error.to_string()),
            }
        }
    }
    async fn command(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::DismissNotice => {
                self.snapshot.notice = None;
                return Ok(());
            }
            Command::Context(context) => {
                self.context = context;
                self.active_account = None;
                return Ok(());
            }
            Command::Refresh => {
                self.refresh().await;
                return Ok(());
            }
            Command::ScheduleRuns(id) => {
                let current = self.snapshot.schedule_runs.as_ref().map(|s| &s.schedule_id);
                if current != id.as_ref() {
                    self.snapshot.schedule_runs = id.map(|schedule_id| ScheduleRuns {
                        schedule_id,
                        runs: None,
                        error: None,
                    });
                }
                self.refresh_schedule_runs().await;
                return Ok(());
            }
            Command::Cleanup => {
                // A previous cleanup's notice never stands beside this one's outcome.
                self.snapshot.notice = None;
                // One identity per confirmed cleanup; it is never sent again.
                let request = uuid::Uuid::new_v4();
                let notice =
                    cleanup_outcome(self.backend.cleanup_runs(request, CLEANUP_DAYS).await)?;
                self.snapshot.notice = Some(notice);
            }
            Command::Save(kind, arguments) => {
                let notice = save_notice(kind, &arguments);
                self.edit
                    .prepare(kind, arguments)
                    .map_err(|e| e.to_string())?;
                match self.edit.dispatch(&self.backend).await {
                    Ok(_) => {}
                    Err(EditError::Backend(BackendError::Rejected)) => {
                        // A definitive rejection applied nothing: release the lock
                        // and keep the draft open for correction.
                        self.edit.discard_explicitly().map_err(|e| e.to_string())?;
                        return Err("The backend rejected this change, for example an invalid cron expression or timezone. Your draft is still open.".into());
                    }
                    Err(error) => return Err(error.to_string()),
                }
                self.snapshot.notice = Some(notice.into());
                self.edit.clear_acknowledged().map_err(|e| e.to_string())?;
                let created = match kind {
                    ChangeKind::CreateJob => Some("jobsCreated"),
                    ChangeKind::CreateSchedule => Some("schedulesCreated"),
                    _ => None,
                };
                if let Some(field) = created {
                    // The change is confirmed; a lost counter update is not an error.
                    let _ = self.backend.increment_stat(field).await;
                }
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
            Command::DiscardTrigger => {
                self.trigger
                    .discard_explicitly()
                    .map_err(|e| e.to_string())?;
                self.snapshot.notice = Some(
                    "The local run request was discarded. If the server received it, the run still appears in History."
                        .into(),
                );
            }
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
                // A queued Stop wins over a tick that is ready at the same moment, so quit
                // starts no extra work. Commands keep their order among themselves.
                biased;
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
                    let context = self.dispatch_context().await;
                    self.snapshot.dispatch_error = self.engine.tick(context).await.err().map(|error| error.to_string());
                    polls += 1;
                    // A claimed, finished or cancelled run changes History at once.
                    if polls % 5 == 0 || self.engine.phase() != before { self.refresh().await; }
                    // A "queued" or "waiting to stop" notice is stale once the run settles.
                    if self.engine.phase() == Phase::Idle && before != Phase::Idle { self.snapshot.notice = None; }
                }
            }
            self.publish(&target);
        }
    }
}
