//! One explicit user turn. Notifications coalesce; private output is never logged.
pub mod qualification;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::{AbortHandle, Abortable, BoxFuture};
use github_copilot_sdk::session::Session;
use tokio::sync::watch;

use crate::copilot_backend::{
    CopilotBackend, NativeRequest, RecordStatus, ReviewSnapshot, failure_code,
};
use crate::copilot_runs::{Context, Execution, Failure, Phase, SendRequest};
use crate::copilot_sdk::{AssistantRuntime, EventAction, apply_event};

pub struct RunOptions {
    pub context: Context,
    pub prompt: String,
    pub saved: bool,
    pub review: Option<ReviewSnapshot>,
    pub backend: Option<CopilotBackend>,
    pub executable: PathBuf,
    pub owned_directory: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    NotRequested,
    Pending,
    Saving,
    Saved,
    Unconfirmed,
    Reconciled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Preparing,
    Running,
    Stopping,
    Finished,
}

/// No Debug: this is private assistant content.
pub struct Snapshot {
    pub phase: Phase,
    pub progress: Progress,
    pub output: String,
    pub failure: Option<Failure>,
    pub result_id: Option<String>,
    pub save: SaveState,
    pub busy: bool,
    pub cleanup_warning: bool,
}

struct CapturedOutcome {
    id: String,
    output: String,
    failure: Option<&'static str>,
}
struct State {
    execution: Execution,
    progress: Progress,
    save: SaveState,
    captured: Option<Arc<CapturedOutcome>>,
    busy: bool,
    cleanup_warning: bool,
}
struct Shared {
    fixture: Option<Arc<crate::copilot_fixture::Fixture>>,
    state: Mutex<State>,
    runtime: Mutex<Option<OwnedRuntime>>,
    updates: watch::Sender<()>,
    request: Option<Arc<NativeRequest>>,
    backend: Option<CopilotBackend>,
}

struct OwnedRuntime {
    client: Arc<AssistantRuntime>,
    state_directory: Option<tempfile::TempDir>,
}
fn close_temporary_directory(directory: tempfile::TempDir) -> bool {
    let path = directory.path().to_owned();
    if directory.close().is_ok() {
        return true;
    }
    // force_stop sends a kill signal; Windows can briefly retain file handles.
    // This runs on the owned I/O task, after stop/abort, never in the view render.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => return true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(_) => return false,
        }
    }
}
impl OwnedRuntime {
    fn close_directory(&mut self) -> bool {
        self.state_directory
            .take()
            .is_none_or(close_temporary_directory)
    }
}
impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        self.client.force_stop();
        if !self.close_directory() {
            log::warn!("Assistant temporary state could not be removed.");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
impl Shared {
    fn changed(&self) {
        self.updates.send_replace(());
    }
    fn failed(&self, failure: Failure) {
        let mut state = lock(&self.state);
        let key = state.execution.key();
        state.execution.failed(key, failure);
        drop(state);
        self.changed();
    }
    fn confirm_cancel(&self) {
        let mut state = lock(&self.state);
        let key = state.execution.key();
        state.execution.abort_confirmed(key);
        drop(state);
        self.changed();
    }
}

pub struct RunHandle {
    shared: Arc<Shared>,
    cancelled: watch::Sender<bool>,
    abort: AbortHandle,
}
impl RunHandle {
    pub fn snapshot(&self) -> Snapshot {
        let state = lock(&self.shared.state);
        Snapshot {
            phase: state.execution.phase(),
            progress: state.progress,
            output: state.execution.output().into(),
            failure: state.execution.failure(),
            result_id: state.execution.result_id().map(str::to_owned),
            save: state.save,
            busy: state.busy,
            cleanup_warning: state.cleanup_warning,
        }
    }
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.shared.updates.subscribe()
    }
    pub fn cancel(&self) -> bool {
        let changed = lock(&self.shared.state).execution.cancel();
        if changed {
            self.cancelled.send_replace(true);
            self.shared.changed();
        }
        changed
    }
    /// Only the captured storage operation can run again. This never starts an SDK turn.
    pub fn retry_save(&self) -> Option<BoxFuture<'static, ()>> {
        let mut state = lock(&self.shared.state);
        if state.busy || state.save != SaveState::Unconfirmed {
            return None;
        }
        state.save = SaveState::Saving;
        let captured = state.captured.clone();
        drop(state);
        self.shared.changed();
        let shared = self.shared.clone();
        Some(Box::pin(async move {
            if let Some(captured) = captured {
                persist(&shared, captured).await;
            } else {
                reconcile(&shared).await;
            }
        }))
    }
}
impl Drop for RunHandle {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
        if let Some(runtime) = lock(&self.shared.runtime).as_ref() {
            runtime.client.force_stop();
        }
        self.abort.abort();
    }
}

pub fn start(options: RunOptions) -> Result<(RunHandle, impl Future<Output = ()> + Send), Failure> {
    let fixture = crate::copilot_fixture::load(&options.owned_directory)?;
    if let Some(fixture) = &fixture {
        fixture.identity(&options.context.account, Some(&options.context.model))?;
        if options
            .backend
            .as_ref()
            .is_some_and(|backend| !fixture.matches_backend(backend.deployment()))
        {
            return Err(Failure::InvalidContext);
        }
    }
    if !options.saved && options.review.is_some() {
        return Err(Failure::InvalidContext);
    }
    let execution = Execution::new(options.context, options.prompt, options.saved)?;
    if options.saved && options.backend.is_none() {
        return Err(Failure::PersistenceFailed);
    }
    let request = if options.saved {
        Some(Arc::new(NativeRequest::new(
            &execution,
            options.review.as_ref(),
        )?))
    } else {
        None
    };
    let (updates, _) = watch::channel(());
    let (cancelled, cancel) = watch::channel(false);
    let shared = Arc::new(Shared {
        fixture,
        state: Mutex::new(State {
            execution,
            progress: Progress::Preparing,
            save: if options.saved {
                SaveState::Pending
            } else {
                SaveState::NotRequested
            },
            captured: None,
            busy: true,
            cleanup_warning: false,
        }),
        runtime: Mutex::new(None),
        updates,
        request,
        backend: options.backend,
    });
    let (abort, registration) = AbortHandle::new_pair();
    let handle = RunHandle {
        shared: shared.clone(),
        cancelled,
        abort,
    };
    let future = async move {
        let _ = Abortable::new(
            drive(shared, options.executable, options.owned_directory, cancel),
            registration,
        )
        .await;
    };
    Ok((handle, future))
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn drive(
    shared: Arc<Shared>,
    executable: PathBuf,
    owned: PathBuf,
    mut cancel: watch::Receiver<bool>,
) {
    let key = lock(&shared.state).execution.key();
    if let (Some(backend), Some(request)) = (&shared.backend, &shared.request) {
        match backend.begin(request).await {
            Ok(receipt) if receipt.created && receipt.status == RecordStatus::Pending => {
                lock(&shared.state)
                    .execution
                    .record_created(key, receipt.id);
            }
            receipt => {
                let mut state = lock(&shared.state);
                state.execution.record_unknown(key);
                if let Ok(receipt) = receipt {
                    state.execution.record_created(key, receipt.id);
                }
                state.save = SaveState::Unconfirmed;
                state.busy = false;
                state.progress = Progress::Finished;
                drop(state);
                shared.changed();
                return;
            }
        }
    }
    let send = lock(&shared.state).execution.begin_send();
    if let Ok(send) = send {
        let result = run_transport(&shared, executable, owned, send, &mut cancel).await;
        if let Err(failure) = result {
            shared.failed(failure);
        }
    }
    shared.confirm_cancel();
    {
        lock(&shared.state).progress = Progress::Stopping;
    }
    shared.changed();
    let runtime = lock(&shared.runtime).take();
    if let Some(mut runtime) = runtime {
        let stopped = runtime.client.stop().await.is_ok();
        let cleaned = runtime.close_directory();
        if !stopped || !cleaned {
            lock(&shared.state).cleanup_warning = true;
        }
    }
    let captured = {
        let mut state = lock(&shared.state);
        let captured = state.execution.result_id().map(|id| {
            Arc::new(CapturedOutcome {
                id: id.into(),
                output: state.execution.output().into(),
                failure: if state.execution.phase() == Phase::Cancelled {
                    Some("cancelled")
                } else {
                    state.execution.failure().map(failure_code)
                },
            })
        });
        state.captured = captured.clone();
        captured
    };
    if let Some(captured) = captured {
        persist(&shared, captured).await;
    }
    {
        let mut state = lock(&shared.state);
        state.busy = false;
        state.progress = Progress::Finished;
    }
    shared.changed();
}

async fn run_transport(
    shared: &Arc<Shared>,
    executable: PathBuf,
    owned: PathBuf,
    send: SendRequest,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), Failure> {
    if *cancel.borrow() {
        shared.confirm_cancel();
        return Ok(());
    }
    if let (Some(backend), Some(request)) = (&shared.backend, &shared.request) {
        let id = lock(&shared.state)
            .execution
            .result_id()
            .map(str::to_owned)
            .ok_or(Failure::RecordUnknown)?;
        let receipt = backend
            .claim(request, &id)
            .await
            .map_err(|_| Failure::SendUnknown)?;
        if !receipt.claimed || receipt.status != RecordStatus::Running {
            return Err(Failure::SendUnknown);
        }
    }
    if *cancel.borrow() {
        shared.confirm_cancel();
        return Ok(());
    }
    let stop_prepare = Arc::new(AtomicBool::new(false));
    let preparing = crate::copilot_runtime::prepare_with_cancel(
        executable,
        owned.join("runtime"),
        stop_prepare.clone(),
    );
    tokio::pin!(preparing);
    let program = tokio::select! {
        biased;
        _=cancelled(cancel)=>{
            stop_prepare.store(true,Ordering::Release);
            let stopped=preparing.await;
            if matches!(stopped,Err(crate::copilot_runtime::InstallError::CleanupFailed {..})) {lock(&shared.state).cleanup_warning=true;}
            if !matches!(stopped,Ok(_) | Err(crate::copilot_runtime::InstallError::Cancelled)) {
                return Err(Failure::CancellationUnknown);
            }
            shared.confirm_cancel();return Ok(());
        }
        result=&mut preparing=>{
            if matches!(result,Err(crate::copilot_runtime::InstallError::CleanupFailed {..})) {lock(&shared.state).cleanup_warning=true;}
            result.map_err(|_|Failure::ServiceUnavailable)?
        },
    };
    let credential = tokio::select! {
        biased;
        _=cancelled(cancel)=>{shared.confirm_cancel();return Ok(());}
        token=async {if shared.fixture.is_some() {Ok(None)} else {crate::gh::auth_token(&send.context.account).await.map(Some).map_err(|_|Failure::AuthenticationRequired)}}=>token?,
    };
    let turns = owned.join("turns");
    crate::copilot_sdk::private_directory(&turns).map_err(|_| Failure::ServiceUnavailable)?;
    let turn_directory = tempfile::Builder::new()
        .prefix("turn-")
        .tempdir_in(turns)
        .map_err(|_| Failure::ServiceUnavailable)?;
    let runtime = tokio::select! {
        biased;
        _=cancelled(cancel)=>{shared.confirm_cancel();return Ok(());}
        result=AssistantRuntime::start_with_fixture(program.program(),turn_directory.path(),credential,shared.fixture.clone())=>Arc::new(result?),
    };
    *lock(&shared.runtime) = Some(OwnedRuntime {
        client: runtime.clone(),
        state_directory: Some(turn_directory),
    });
    let session = tokio::select! {
        biased;
        _=cancelled(cancel)=>{shared.confirm_cancel();return Ok(());}
        result=runtime.create_session(&send.context.account,&send.context.model)=>result?,
    };
    let mut events = session.subscribe();
    // Consume deltas while the send acknowledgement is pending. A slow or lost
    // acknowledgement must not block rendering or overflow the event receiver.
    let sending = tokio::time::timeout(Duration::from_secs(30), session.send(send.prompt));
    tokio::pin!(sending);
    let mut acknowledged = false;
    let deadline = tokio::time::sleep(Duration::from_secs(30 * 60));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _=cancelled(cancel)=>return abort_turn(shared,&session).await,
            _=&mut deadline=>{runtime.force_stop();return Err(Failure::IncompleteStream);}
            result=&mut sending, if !acknowledged=>{
                if !matches!(result,Ok(Ok(_))) {runtime.force_stop();return Err(Failure::SendUnknown);}
                acknowledged=true;lock(&shared.state).progress=Progress::Running;shared.changed();
            }
            event=events.recv()=>{
                let event=match event {Ok(event)=>event,Err(_)=>{runtime.force_stop();return Err(Failure::IncompleteStream);}};
                let action=apply_event(&mut lock(&shared.state).execution,send.key,&event);
                match action {
                    EventAction::Ignored=>{},
                    EventAction::Changed=>{lock(&shared.state).progress=Progress::Running;shared.changed();}
                    // A final message followed by idle proves completion even
                    // when its earlier send acknowledgement has not arrived.
                    EventAction::Finished=>{shared.changed();return Ok(());}
                    EventAction::AbortRequired=>{runtime.force_stop();return Err(Failure::IncompleteStream);}
                }
            }
        }
    }
}

async fn abort_turn(shared: &Shared, session: &Session) -> Result<(), Failure> {
    if matches!(
        tokio::time::timeout(Duration::from_secs(15), session.abort()).await,
        Ok(Ok(()))
    ) {
        shared.confirm_cancel();
        Ok(())
    } else {
        Err(Failure::CancellationUnknown)
    }
}

async fn persist(shared: &Shared, captured: Arc<CapturedOutcome>) {
    let (Some(backend), Some(request)) = (&shared.backend, &shared.request) else {
        return;
    };
    lock(&shared.state).save = SaveState::Saving;
    shared.changed();
    let result = backend
        .finish(request, &captured.id, &captured.output, captured.failure)
        .await;
    let mut state = lock(&shared.state);
    let key = state.execution.key();
    if result.is_ok() {
        state.save = SaveState::Saved;
        state.execution.persisted(key, &captured.id);
    } else {
        state.save = SaveState::Unconfirmed;
        state.execution.persistence_failed(key);
    }
    drop(state);
    shared.changed();
}

async fn reconcile(shared: &Shared) {
    let (Some(backend), Some(request)) = (&shared.backend, &shared.request) else {
        return;
    };
    let result = backend.reconcile(request).await;
    let mut state = lock(&shared.state);
    let key = state.execution.key();
    if let Ok(Some((id, _))) = result {
        state.execution.record_created(key, id);
        state.save = SaveState::Reconciled;
    } else {
        state.save = SaveState::Unconfirmed;
    }
    drop(state);
    shared.changed();
}

#[cfg(test)]
mod tests;
